#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>

extern int horizon_ios_run(const char *resources, const char *game_dir);
extern const char *horizon_ios_fetch(const char *version, const char *jar_dir,
                                     const char *store_dir);
extern const char *horizon_ios_assemble(const char *bundle, const char *client_jar,
                                      const char *index, const char *store,
                                      const char *out);
extern const char *horizon_ios_check_resources(const char *resources);

// The engine app has no UIKit shell of its own: winit's iOS backend calls
// `UIApplicationMain` and builds the window from Rust. So there are two launch
// paths and exactly one of them runs per process:
//
//   resources ready  -> hand straight to the engine
//   not ready        -> show a native "preparing" screen that fetches and
//                       assembles, then ask the player to reopen
//
// The second path exists because a first run has to download the player's own
// copy (about 140 MB) and untar it into a 250 MB tree. Doing that inside
// `main()` would trip the launch watchdog: the app is killed at around 20
// seconds. A run loop has to exist first.

static NSString *gGameDir = nil;
static NSString *gResources = nil;
static NSString *gLogPath = nil;
static BOOL gPreparing = NO;

static void Log(NSString *format, ...) {
    va_list arguments;
    va_start(arguments, format);
    NSString *line = [[NSString alloc] initWithFormat:format arguments:arguments];
    va_end(arguments);
    NSLog(@"Horizon: %@", line);
    if (gLogPath == nil) {
        return;
    }
    NSString *stamped = [NSString stringWithFormat:@"[%@] %@\n", [NSDate date], line];
    NSFileHandle *handle = [NSFileHandle fileHandleForWritingAtPath:gLogPath];
    if (handle == nil) {
        [stamped writeToFile:gLogPath atomically:YES encoding:NSUTF8StringEncoding error:nil];
    } else {
        [handle seekToEndOfFile];
        [handle writeData:[stamped dataUsingEncoding:NSUTF8StringEncoding]];
        [handle closeFile];
    }
}

static NSString *DocumentsPath(void) {
    NSArray<NSString *> *documents =
        NSSearchPathForDirectoriesInDomains(NSDocumentDirectory, NSUserDomainMask, YES);
    return documents.firstObject ?: NSTemporaryDirectory();
}

/// Every path the two steps use, in one place.
@interface FirstRun : NSObject
@property(nonatomic, copy) NSString *gameDir;
@property(nonatomic, copy) NSString *resources;
@property(nonatomic, copy) NSString *ship;
@property(nonatomic, copy) NSString *jarDir;
@property(nonatomic, copy) NSString *assetsDir;
@property(nonatomic, copy) NSString *clientJar;
@property(nonatomic, copy) NSString *assetIndex;
@property(nonatomic, copy) NSString *assetStore;
@end

@implementation FirstRun

- (instancetype)init {
    self = [super init];
    if (self == nil) {
        return nil;
    }
    NSFileManager *manager = [NSFileManager defaultManager];
    _gameDir = DocumentsPath();
    [manager createDirectoryAtPath:_gameDir withIntermediateDirectories:YES attributes:nil error:nil];

    _resources = [_gameDir stringByAppendingPathComponent:@"horizon-client/26.3"];
    [manager createDirectoryAtPath:_resources
        withIntermediateDirectories:YES
                         attributes:nil
                              error:nil];
    // The resource tree is rebuilt from the player's own files, so it must not
    // spend their iCloud quota.
    NSURL *url = [NSURL fileURLWithPath:_resources isDirectory:YES];
    [url setResourceValue:@YES forKey:NSURLIsExcludedFromBackupKey error:nil];

    // A bundle the player imported wins over the one shipped inside the app.
    NSString *imported = [_gameDir stringByAppendingPathComponent:@"ship.hzr3"];
    if ([manager fileExistsAtPath:imported]) {
        _ship = imported;
    } else {
        _ship = [[NSBundle mainBundle] pathForResource:@"ship" ofType:@"hzr3"];
    }

    _jarDir = [_gameDir stringByAppendingPathComponent:@"mc"];
    _assetsDir = [_gameDir stringByAppendingPathComponent:@"assets"];
    _clientJar = [_jarDir stringByAppendingPathComponent:@"client.jar"];
    _assetIndex = [_assetsDir stringByAppendingPathComponent:@"indexes/34.json"];
    _assetStore = [_assetsDir stringByAppendingPathComponent:@"objects"];
    return self;
}

- (BOOL)prepared {
    NSFileManager *manager = [NSFileManager defaultManager];
    return [manager fileExistsAtPath:
                [self.resources stringByAppendingPathComponent:@"horizon-resources.json"]];
}

@end

static NSString *Describe(FirstRun *run) {
    NSFileManager *manager = [NSFileManager defaultManager];
    NSMutableString *text = [NSMutableString string];
    [text appendFormat:@"game dir     %@\n", run.gameDir];
    [text appendFormat:@"resources    %@\n", run.resources];
    [text appendFormat:@"prepared     %@\n", [run prepared] ? @"yes" : @"no"];
    [text appendFormat:@"ship.hzr3    %@\n", run.ship ?: @"(not found)"];
    if (run.ship) {
        NSDictionary *attributes = [manager attributesOfItemAtPath:run.ship error:nil];
        [text appendFormat:@"             %@ bytes\n", attributes[NSFileSize]];
    }
    [text appendFormat:@"client.jar   %@\n", [manager fileExistsAtPath:run.clientJar] ? @"yes" : @"no"];
    [text appendFormat:@"asset index  %@\n", [manager fileExistsAtPath:run.assetIndex] ? @"yes" : @"no"];
    [text appendFormat:@"asset store  %@\n", [manager fileExistsAtPath:run.assetStore] ? @"yes" : @"no"];
    const char *gaps = horizon_ios_check_resources(run.resources.UTF8String);
    [text appendFormat:@"check         %@\n", gaps ? [NSString stringWithUTF8String:gaps] : @"(complete)"];
    return text;
}

@interface PrepareController : UIViewController
@property(nonatomic, strong) UITextView *log;
@property(nonatomic, strong) UILabel *title;
@property(nonatomic, strong) FirstRun *run;
@end

@implementation PrepareController

- (void)viewDidLoad {
    [super viewDidLoad];
    self.view.backgroundColor = [UIColor colorWithWhite:0.06 alpha:1];

    self.title = [[UILabel alloc] initWithFrame:CGRectMake(24, 70, self.view.bounds.size.width - 48, 34)];
    self.title.text = @"Preparing Horizon";
    self.title.textColor = [UIColor whiteColor];
    self.title.font = [UIFont boldSystemFontOfSize:28];
    [self.view addSubview:self.title];

    self.log = [[UITextView alloc] initWithFrame:CGRectMake(20, 120,
                                                           self.view.bounds.size.width - 40,
                                                           self.view.bounds.size.height - 160)];
    self.log.backgroundColor = [UIColor colorWithWhite:0.1 alpha:1];
    self.log.textColor = [UIColor colorWithWhite:0.85 alpha:1];
    self.log.font = [UIFont monospacedSystemFontOfSize:12 weight:UIFontWeightRegular];
    self.log.editable = NO;
    self.log.text = Describe(self.run);
    [self.view addSubview:self.log];
}

- (void)append:(NSString *)text {
    dispatch_async(dispatch_get_main_queue(), ^{
        self.log.text = [self.log.text stringByAppendingFormat:@"%@\n", text];
        UIScrollView *scroll = self.log;
        [scroll scrollRangeToVisible:NSMakeRange(self.log.text.length - 1, 1)];
    });
}

- (void)viewDidAppear:(BOOL)animated {
    [super viewDidAppear:animated];
    static BOOL started = NO;
    if (started) {
        return;
    }
    started = YES;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        [self prepare];
    });
}

- (void)prepare {
    NSFileManager *manager = [NSFileManager defaultManager];
    FirstRun *run = self.run;
    Log(@"preparing: start");
    [self append:@"fetching the player's own files…"];

    if (![manager fileExistsAtPath:run.ship]) {
        [self fail:@"no ship.hzr3 in the app and none imported"];
        return;
    }

    if (![manager fileExistsAtPath:run.clientJar] ||
        ![manager fileExistsAtPath:run.assetIndex]) {
        const char *first = horizon_ios_fetch("26.3", run.jarDir.UTF8String,
                                              run.assetsDir.UTF8String);
        if (first != NULL) {
            [self fail:[NSString stringWithFormat:@"download failed: %s", first]];
            return;
        }
    }
    [self append:@"download done"];
    Log(@"preparing: download done");

    if (![manager fileExistsAtPath:run.clientJar] ||
        ![manager fileExistsAtPath:run.assetIndex] ||
        ![manager fileExistsAtPath:run.assetStore]) {
        [self fail:@"download produced no client.jar, index or objects"];
        return;
    }

    [self append:@"assembling the resource tree…"];
    const char *second = horizon_ios_assemble(run.ship.UTF8String, run.clientJar.UTF8String,
                                              run.assetIndex.UTF8String, run.assetStore.UTF8String,
                                              run.resources.UTF8String);
    if (second != NULL) {
        [self fail:[NSString stringWithFormat:@"assembly failed: %s", second]];
        return;
    }
    Log(@"preparing: assembled");

    const char *gaps = horizon_ios_check_resources(run.resources.UTF8String);
    if (gaps != NULL) {
        [self fail:[NSString stringWithFormat:@"still incomplete: %s", gaps]];
        return;
    }

    [self append:@"done — reopen the app to play"];
    Log(@"preparing: complete");
    dispatch_async(dispatch_get_main_queue(), ^{
        self.title.text = @"Ready";
    });
}

- (void)fail:(NSString *)why {
    Log(@"preparing: %@", why);
    [self append:[NSString stringWithFormat:@"failed: %@", why]];
    [self append:[NSString stringWithFormat:@"log: %@", gLogPath]];
    dispatch_async(dispatch_get_main_queue(), ^{
        self.title.text = @"Could not prepare";
        self.title.textColor = [UIColor systemRedColor];
    });
}

@end

// Info.plist names `NCLRSceneDelegate`, so it has to exist or the prepare screen
// never gets a window. It only builds one while preparing: on the engine path
// winit owns the window and this class must stay out of the way.
@interface NCLRSceneDelegate : UIResponder <UIWindowSceneDelegate>
@property(nonatomic, strong) UIWindow *window;
@end

@implementation NCLRSceneDelegate

- (void)scene:(UIScene *)scene
    willConnectToSession:(UISceneSession *)session
                 options:(UISceneConnectionOptions *)options {
    if (!gPreparing) {
        return;
    }
    if (![scene isKindOfClass:[UIWindowScene class]]) {
        return;
    }
    self.window = [[UIWindow alloc] initWithWindowScene:(UIWindowScene *)scene];
    PrepareController *controller = [[PrepareController alloc] init];
    controller.run = [[FirstRun alloc] init];
    self.window.rootViewController = controller;
    [self.window makeKeyAndVisible];
}

@end

@interface PrepareApp : UIResponder <UIApplicationDelegate>
@property(nonatomic, strong) UIWindow *window;
@end

@implementation PrepareApp

- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary *)options {
    // The window comes from NCLRSceneDelegate, which Info.plist names.
    return YES;
}

@end

int main(int argc, char *argv[]) {
    @autoreleasepool {
        gGameDir = DocumentsPath();
        gLogPath = [gGameDir stringByAppendingPathComponent:@"first-run.log"];
        [@"" writeToFile:gLogPath atomically:YES encoding:NSUTF8StringEncoding error:nil];

        FirstRun *run = [[FirstRun alloc] init];
        gResources = run.resources;

        Log(@"launch: %@", Describe(run));

        setenv("HORIZON_HOME", gGameDir.UTF8String, 1);
        setenv("HORIZON_NO_AUDIO", "1", 1);
        setenv("HORIZON_NO_DISCORD", "1", 1);

        if (![run prepared] || horizon_ios_check_resources(gResources.UTF8String) != NULL) {
            // Nothing to hand the engine yet: prepare with a real run loop, so
            // the launch watchdog never sees a blocked main thread.
            Log(@"launch: resources are not ready, starting the prepare screen");
            gPreparing = YES;
            @autoreleasepool {
                return UIApplicationMain(argc, argv, nil, NSStringFromClass([PrepareApp class]));
            }
        }

        Log(@"launch: resources ready, starting the engine");
        return horizon_ios_run(gResources.UTF8String, gGameDir.UTF8String);
    }
}

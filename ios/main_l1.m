#import <UIKit/UIKit.h>
#import <AVFoundation/AVFoundation.h>
#import <CoreMotion/CoreMotion.h>
#import <GameController/GameController.h>
#import <SceneKit/SceneKit.h>
#import <objc/runtime.h>
#import <Security/Security.h>
#import <CommonCrypto/CommonDigest.h>
#import <AuthenticationServices/AuthenticationServices.h>
#import <SafariServices/SafariServices.h>
#import <stdlib.h>
#include <string.h>
#include <math.h>
#include <stdint.h>

extern int horizon_ios_run(const char *resources, const char *game_dir);
extern void horizon_ios_add_motion(double dx, double dy);
extern void horizon_ios_set_hardware_input(int attached, int mouse);
extern void horizon_ios_hardware_key(uint16_t usage, int pressed, int repeat, uint8_t modifiers);
extern void horizon_ios_hardware_mouse_motion(double dx, double dy, double x, double y);
extern void horizon_ios_hardware_mouse_button(int button, int pressed);
extern void horizon_ios_hardware_mouse_scroll(double dx, double dy);
@class NCLRResourcePreparer;
static NSString *const NCLRPrepareProgressNotification = @"NCLRPrepareProgress";
static NSString *const NCLRPrepareFinishedNotification = @"NCLRPrepareFinished";
static NSString *const NCLRAvatarUpdatedNotification = @"NCLRAvatarUpdated";

static UIVisualEffectView *NCLRAddBlur(UIView *view, BOOL dark) {
    UIBlurEffectStyle style = dark ? UIBlurEffectStyleSystemUltraThinMaterialDark
                                   : UIBlurEffectStyleSystemUltraThinMaterialLight;
    UIVisualEffectView *blur = [[UIVisualEffectView alloc] initWithEffect:[UIBlurEffect effectWithStyle:style]];
    blur.translatesAutoresizingMaskIntoConstraints = NO;
    blur.userInteractionEnabled = NO;
    [view insertSubview:blur atIndex:0];
    [NSLayoutConstraint activateConstraints:@[
        [blur.leadingAnchor constraintEqualToAnchor:view.leadingAnchor],
        [blur.trailingAnchor constraintEqualToAnchor:view.trailingAnchor],
        [blur.topAnchor constraintEqualToAnchor:view.topAnchor],
        [blur.bottomAnchor constraintEqualToAnchor:view.bottomAnchor],
    ]];
    return blur;
}

static char NCLRAvatarRequestKey;

static void NCLRLoadRemoteAvatar(NSString *url, UIImageView *imageView, UILabel *fallback) {
    if (!imageView) return;
    NSString *previous = objc_getAssociatedObject(imageView, &NCLRAvatarRequestKey);
    if ([previous isEqualToString:url]) return;
    objc_setAssociatedObject(imageView, &NCLRAvatarRequestKey, url, OBJC_ASSOCIATION_COPY_NONATOMIC);
    imageView.image = nil;
    imageView.hidden = YES;
    fallback.hidden = NO;
    NSURL *address = url.length ? [NSURL URLWithString:url] : nil;
    if (![address.scheme.lowercaseString isEqualToString:@"https"]) return;

    static NSCache<NSString *, UIImage *> *cache;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        cache = [NSCache new];
        cache.countLimit = 32;
    });
    __weak UIImageView *weakImageView = imageView;
    __weak UILabel *weakFallback = fallback;
    void (^display)(UIImage *) = ^(UIImage *image) {
        UIImageView *target = weakImageView;
        if (!target || ![objc_getAssociatedObject(target, &NCLRAvatarRequestKey) isEqualToString:url]) return;
        target.image = image;
        target.hidden = image == nil;
        weakFallback.hidden = image != nil;
        if (!image) objc_setAssociatedObject(target, &NCLRAvatarRequestKey, nil, OBJC_ASSOCIATION_COPY_NONATOMIC);
    };
    UIImage *cached = [cache objectForKey:url];
    if (cached) {
        display(cached);
        return;
    }
    NSURLRequest *request = [NSURLRequest requestWithURL:address
        cachePolicy:NSURLRequestUseProtocolCachePolicy timeoutInterval:12];
    [[NSURLSession.sharedSession dataTaskWithRequest:request
        completionHandler:^(NSData *data, NSURLResponse *response, NSError *error) {
            UIImage *image = !error && [(NSHTTPURLResponse *)response statusCode] == 200 && data.length > 0
                ? [UIImage imageWithData:data] : nil;
            if (image) [cache setObject:image forKey:url];
            dispatch_async(dispatch_get_main_queue(), ^{ display(image); });
        }] resume];
}
@interface NCLRResourcePreparer : NSObject
+ (instancetype)shared;
- (void)prepareWithProgress:(void (^)(NSString *line))progress
                 completion:(void (^)(NSString *error))completion;
@property (nonatomic, copy) NSString *lastLine;
@property (nonatomic, assign) float progressValue;
@property (nonatomic, assign) BOOL preparing;
@property (atomic, assign) BOOL cancelRequested;
@property (nonatomic, assign) BOOL cancellable;
@property (atomic, strong) NSURLSession *downloadSession;
- (void)cancel;
@end
extern const char *horizon_ios_assemble(const char *bundle, const char *client_jar,
                                        const char *index, const char *store, const char *out);


static NSString *const kNCLRService = @"dev.zrxmaxlnx.nclr.launcher";
static NSString *const kNCLRAccount = @"minecraft-access-token";
static NSString *const kNCLRClientKey = @"NCLRMicrosoftClientID";
static NSString *const kNCLRDefaultClientID = @"da723f82-1e03-48ef-b3ae-313b2e4c34ca";
static NSString *const kNCLRAccountNameKey = @"NCLRAccountName";
static NSString *const kNCLRAccountUUIDKey = @"NCLRAccountUUID";

static NSString *NCLRAvatarURLForName(NSString *name) {
    NSCharacterSet *invalid = [[NSCharacterSet characterSetWithCharactersInString:
        @"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_"] invertedSet];
    if (name.length == 0 || name.length > 16 || [name rangeOfCharacterFromSet:invalid].location != NSNotFound) return nil;
    return [NSString stringWithFormat:@"https://minotar.net/helm/%@/128", name];
}

@class NCLRRootController;
static BOOL NCLRDarkMode = YES;
static NSString *const kNCLRThemeKey = @"NCLRThemeDark";

static void NCLRThemeLoad(void) {
    NSNumber *saved = [[NSUserDefaults standardUserDefaults] objectForKey:kNCLRThemeKey];
    NCLRDarkMode = saved == nil ? YES : saved.boolValue;
}

static void NCLRThemeSet(BOOL dark) {
    NCLRDarkMode = dark;
    [[NSUserDefaults standardUserDefaults] setBool:dark forKey:kNCLRThemeKey];
}

/// The launcher palette is authored for the dark theme. Light mode inverts
/// near-neutral colours so backgrounds, cards, text and hairlines flip
/// together; saturated accents keep their hue.
static UIColor *NCLRColor(CGFloat r, CGFloat g, CGFloat b, CGFloat a) {
    if (!NCLRDarkMode) {
        if (g >= 0.65 && g - r >= 0.22 && g - b >= 0.15) {
            return [UIColor colorWithRed:0.08 green:0.39 blue:0.23 alpha:a];
        }
        CGFloat hi = MAX(r, MAX(g, b));
        CGFloat lo = MIN(r, MIN(g, b));
        if (hi - lo <= 0.10 || hi <= 0.12) {
            if (hi >= 0.30 && hi < 0.86) {
                // Mid-grey secondary text: inversion leaves it at 50% grey on
                // white, which is barely readable. Pull it down to ink.
                CGFloat ink = 0.16 + 0.10 * (1.0 - hi);
                r = ink; g = ink; b = ink;
            } else {
                r = 1.0 - r; g = 1.0 - g; b = 1.0 - b;
            }
        }
    }
    return [UIColor colorWithRed:r green:g blue:b alpha:a];
}

static UIColor *NCLRSurface(void) {
    return NCLRDarkMode ? [UIColor colorWithRed:0.09 green:0.12 blue:0.15 alpha:1]
                        : [UIColor colorWithRed:0.99 green:1 blue:1 alpha:1];
}

static UIColor *NCLRCanvas(void) {
    return NCLRDarkMode ? [UIColor colorWithRed:0.045 green:0.065 blue:0.085 alpha:1]
                        : [UIColor colorWithRed:0.89 green:0.94 blue:0.94 alpha:1];
}

static UIColor *NCLRBorder(void) {
    return NCLRDarkMode ? [UIColor colorWithRed:0.20 green:0.26 blue:0.30 alpha:1]
                        : [UIColor colorWithRed:0.73 green:0.80 blue:0.82 alpha:1];
}

static UIColor *NCLRAccent(void) {
    return NCLRDarkMode ? [UIColor colorWithRed:0.30 green:0.85 blue:0.68 alpha:1]
                        : [UIColor colorWithRed:0.05 green:0.39 blue:0.31 alpha:1];
}

static BOOL NCLRTokenExpired(NSString *token) {
    NSArray<NSString *> *parts = [token componentsSeparatedByString:@"."];
    if (parts.count != 3) return NO;
    NSString *payload = [[parts[1] stringByReplacingOccurrencesOfString:@"-" withString:@"+"]
                                      stringByReplacingOccurrencesOfString:@"_" withString:@"/"];
    NSUInteger padded = ((payload.length + 3) / 4) * 4;
    payload = [payload stringByPaddingToLength:padded withString:@"=" startingAtIndex:0];
    NSData *data = [[NSData alloc] initWithBase64EncodedString:payload options:0];
    id claims = data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
    id expiry = [claims isKindOfClass:NSDictionary.class] ? claims[@"exp"] : nil;
    return [expiry isKindOfClass:NSNumber.class] &&
        [expiry doubleValue] <= NSDate.date.timeIntervalSince1970 + 60;
}

static NSString *NCLRString(id value) {
    return [value isKindOfClass:[NSString class]] ? value : nil;
}

static NSString *NCLRDocuments(void) {
    NSArray<NSString *> *paths =
        NSSearchPathForDirectoriesInDomains(NSDocumentDirectory, NSUserDomainMask, YES);
    return paths.firstObject ?: NSTemporaryDirectory();
}

static NSString *NCLRWasmRefusalText(void) {
    NSString *path = [NCLRDocuments() stringByAppendingPathComponent:@"nclr-wasm-refusals.txt"];
    NSString *text = [NSString stringWithContentsOfFile:path
                                              encoding:NSUTF8StringEncoding
                                                 error:nil];
    return [text stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]];
}

static void NCLRCreateDirectory(NSString *path) {
    [[NSFileManager defaultManager] createDirectoryAtPath:path
                              withIntermediateDirectories:YES
                                               attributes:nil
                                                    error:nil];
}

static NSString *NCLRKeychainRead(void) {
    NSDictionary *query = @{
        (__bridge id)kSecClass : (__bridge id)kSecClassGenericPassword,
        (__bridge id)kSecAttrService : kNCLRService,
        (__bridge id)kSecAttrAccount : kNCLRAccount,
        (__bridge id)kSecReturnData : @YES,
        (__bridge id)kSecMatchLimit : (__bridge id)kSecMatchLimitOne,
    };
    CFTypeRef result = NULL;
    if (SecItemCopyMatching((__bridge CFDictionaryRef)query, &result) != errSecSuccess ||
        result == NULL) {
        return nil;
    }
    NSData *data = (__bridge_transfer NSData *)result;
    return [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
}

static void NCLRKeychainWrite(NSString *value) {
    NSDictionary *query = @{
        (__bridge id)kSecClass : (__bridge id)kSecClassGenericPassword,
        (__bridge id)kSecAttrService : kNCLRService,
        (__bridge id)kSecAttrAccount : kNCLRAccount,
    };
    if (value.length == 0) {
        SecItemDelete((__bridge CFDictionaryRef)query);
        return;
    }
    NSData *data = [value dataUsingEncoding:NSUTF8StringEncoding];
    NSDictionary *update = @{(__bridge id)kSecValueData : data};
    OSStatus status = SecItemUpdate((__bridge CFDictionaryRef)query,
                                    (__bridge CFDictionaryRef)update);
    if (status == errSecItemNotFound) {
        NSMutableDictionary *add = [query mutableCopy];
        add[(__bridge id)kSecValueData] = data;
        add[(__bridge id)kSecAttrAccessible] =
            (__bridge id)kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly;
        SecItemAdd((__bridge CFDictionaryRef)add, NULL);
    }
}

#pragma mark - Models

@interface NCLRVersion : NSObject
@property (nonatomic, copy) NSString *identifier;
@property (nonatomic, copy) NSString *name;
@property (nonatomic, copy) NSString *gameVersion;
@property (nonatomic, copy) NSString *loader;
@property (nonatomic, assign) BOOL installed;
- (NSDictionary *)dictionary;
+ (instancetype)fromDictionary:(NSDictionary *)dictionary;
@end

@implementation NCLRVersion
- (NSDictionary *)dictionary {
    return @{
        @"identifier" : self.identifier ?: @"",
        @"name" : self.name ?: @"",
        @"gameVersion" : self.gameVersion ?: @"",
        @"loader" : self.loader ?: @"Vanilla",
        @"installed" : @(self.installed),
    };
}
+ (instancetype)fromDictionary:(NSDictionary *)dictionary {
    NCLRVersion *version = [[NCLRVersion alloc] init];
    version.identifier = NCLRString(dictionary[@"identifier"]) ?: @"";
    version.name = NCLRString(dictionary[@"name"]) ?: version.identifier;
    version.gameVersion = NCLRString(dictionary[@"gameVersion"]) ?: version.identifier;
    version.loader = NCLRString(dictionary[@"loader"]) ?: @"Vanilla";
    version.installed = [dictionary[@"installed"] boolValue];
    return version;
}
@end


static NSString *NCLRModIdentifier(NSString *path) {
    // `.hmod` manifests are deflated; the launcher cannot inflate them without
    // linking zlib into the app target. Horizon's packer names every archive
    // `<id>.hmod`, so the file stem is the manifest id.
    (void)path;
    return nil;
}


@interface NCLRDisplayCadence : NSObject
- (void)displayTick:(CADisplayLink *)link;
@end

@implementation NCLRDisplayCadence
- (void)displayTick:(CADisplayLink *)link {
    // Keep the iOS display cadence request active. Metal presents via VSync.
    (void)link;
}
@end

// Runs on the main queue. Device motion is fused and low-pass filtered, then
// integrated with hardware timestamps so frame drops do not change the turn.
static CMMotionManager *NCLRStartGyro(UIWindowScene *scene, CGFloat sensitivity,
                                       BOOL invertX, BOOL invertY) {
    CMMotionManager *motion = [CMMotionManager new];
    if (!motion.deviceMotionAvailable) return nil;
    motion.deviceMotionUpdateInterval = 1.0 / 120.0;
    __block NSTimeInterval previous = 0;
    __block double smoothYaw = 0.0;
    __block double smoothPitch = 0.0;
    __block UIInterfaceOrientation lastOrientation = UIInterfaceOrientationUnknown;
    __weak UIWindowScene *gameScene = scene;
    [motion startDeviceMotionUpdatesToQueue:NSOperationQueue.mainQueue
                                withHandler:^(CMDeviceMotion *sample, NSError *error) {
        if (error || !sample || UIApplication.sharedApplication.applicationState != UIApplicationStateActive) {
            previous = 0;
            smoothYaw = smoothPitch = 0.0;
            lastOrientation = UIInterfaceOrientationUnknown;
            return;
        }
        UIInterfaceOrientation orientation = gameScene.interfaceOrientation;
        if (!UIInterfaceOrientationIsLandscape(orientation)) return;
        if (orientation != lastOrientation) {
            lastOrientation = orientation;
            previous = 0;
            smoothYaw = smoothPitch = 0.0;
            return;
        }
        NSTimeInterval elapsed = previous > 0 ? sample.timestamp - previous : 0;
        previous = sample.timestamp;
        if (elapsed <= 0 || elapsed > 0.04 ||
            NSProcessInfo.processInfo.systemUptime - sample.timestamp > 0.08) {
            previous = 0;
            smoothYaw = smoothPitch = 0.0;
            return;
        }
        double direction = orientation == UIInterfaceOrientationLandscapeLeft ? 1.0 : -1.0;
        double yawRate = sample.rotationRate.x;
        double pitchRate = sample.rotationRate.y;
        // Follow quick turns closely, while settling sensor noise when still.
        double speed = hypot(yawRate, pitchRate);
        double alpha = fmin(0.82, 0.18 + speed * 0.22);
        smoothYaw += alpha * (yawRate - smoothYaw);
        smoothPitch += alpha * (pitchRate - smoothPitch);
        const double deadzone = 0.006;
        double yaw = fabs(smoothYaw) < deadzone ? 0.0 : smoothYaw;
        double pitch = fabs(smoothPitch) < deadzone ? 0.0 : smoothPitch;
        const double radiansToMouseUnits = 180.0 / (M_PI * 0.15);
        if (yaw || pitch) {
            double scale = direction * elapsed * sensitivity * radiansToMouseUnits;
            double dx = (invertX ? -yaw : yaw) * scale;
            double dy = (invertY ? -pitch : pitch) * scale;
            const double maxStep = 24.0;
            dx = fmax(-maxStep, fmin(maxStep, dx));
            dy = fmax(-maxStep, fmin(maxStep, dy));
            horizon_ios_add_motion(dx, dy);
        }
    }];
    return motion;
}

static void NCLRRunOnMain(dispatch_block_t block) {
    if (NSThread.isMainThread) {
        block();
    } else {
        dispatch_async(dispatch_get_main_queue(), block);
    }
}

@interface NCLRHardwareInput : NSObject
@property (nonatomic, strong) GCKeyboard *keyboard;
@property (nonatomic, strong) GCMouse *mouse;
@property (nonatomic, strong) NSMutableSet<NSNumber *> *keysDown;
@property (nonatomic, assign) BOOL capsLock;
@property (nonatomic, assign) BOOL cursorReady;
@property (nonatomic, assign) CGPoint cursor;
@property (nonatomic, assign) CGSize cursorBounds;
@property (nonatomic, assign) BOOL running;
+ (instancetype)shared;
- (void)start;
- (void)stop;
@end

@implementation NCLRHardwareInput
+ (instancetype)shared {
    static NCLRHardwareInput *input;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ input = [[NCLRHardwareInput alloc] init]; });
    return input;
}

- (instancetype)init {
    self = [super init];
    if (self) _keysDown = [NSMutableSet set];
    return self;
}

- (void)start {
    if (self.running) return;
    self.running = YES;
    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    [center addObserver:self selector:@selector(devicesChanged:)
                   name:GCKeyboardDidConnectNotification object:nil];
    [center addObserver:self selector:@selector(devicesChanged:)
                   name:GCKeyboardDidDisconnectNotification object:nil];
    [center addObserver:self selector:@selector(devicesChanged:)
                   name:GCMouseDidConnectNotification object:nil];
    [center addObserver:self selector:@selector(devicesChanged:)
                   name:GCMouseDidDisconnectNotification object:nil];
    [center addObserver:self selector:@selector(devicesChanged:)
                   name:UIApplicationDidBecomeActiveNotification object:nil];
    [self refreshDevices];
}

- (void)stop {
    if (!self.running) return;
    self.running = NO;
    [NSNotificationCenter.defaultCenter removeObserver:self];
    [self detachKeyboard:YES];
    [self detachMouse];
    horizon_ios_set_hardware_input(0, 0);
    self.cursorReady = NO;
}

- (void)devicesChanged:(NSNotification *)note {
    (void)note;
    __weak typeof(self) weakSelf = self;
    NCLRRunOnMain(^{ [weakSelf refreshDevices]; });
}

- (void)refreshDevices {
    GCKeyboard *keyboard = GCKeyboard.coalescedKeyboard;
    if (keyboard != self.keyboard) {
        [self detachKeyboard:YES];
        self.keyboard = keyboard;
        [self attachKeyboard:keyboard];
    }
    GCMouse *mouse = GCMouse.current;
    if (mouse != self.mouse) {
        [self detachMouse];
        self.mouse = mouse;
        [self attachMouse:mouse];
    }
    BOOL hasKeyboard = self.keyboard != nil;
    BOOL hasMouse = self.mouse != nil;
    horizon_ios_set_hardware_input((hasKeyboard || hasMouse) ? 1 : 0, hasMouse ? 1 : 0);
}

- (uint8_t)modifiersForInput:(GCKeyboardInput *)input {
    GCControllerButtonInput *leftShift = [input buttonForKeyCode:GCKeyCodeLeftShift];
    GCControllerButtonInput *rightShift = [input buttonForKeyCode:GCKeyCodeRightShift];
    GCControllerButtonInput *leftControl = [input buttonForKeyCode:GCKeyCodeLeftControl];
    GCControllerButtonInput *rightControl = [input buttonForKeyCode:GCKeyCodeRightControl];
    GCControllerButtonInput *leftAlt = [input buttonForKeyCode:GCKeyCodeLeftAlt];
    GCControllerButtonInput *rightAlt = [input buttonForKeyCode:GCKeyCodeRightAlt];
    GCControllerButtonInput *leftCommand = [input buttonForKeyCode:GCKeyCodeLeftGUI];
    GCControllerButtonInput *rightCommand = [input buttonForKeyCode:GCKeyCodeRightGUI];
    BOOL shift = leftShift.isPressed || rightShift.isPressed;
    BOOL control = leftControl.isPressed || rightControl.isPressed;
    BOOL alternate = leftAlt.isPressed || rightAlt.isPressed;
    BOOL command = leftCommand.isPressed || rightCommand.isPressed;
    return (shift ? 1 : 0) | (control ? 2 : 0) | (alternate ? 4 : 0) |
           (command ? 8 : 0) | (self.capsLock ? 16 : 0);
}

- (void)attachKeyboard:(GCKeyboard *)keyboard {
    if (!keyboard) return;
    __weak typeof(self) weakSelf = self;
    keyboard.keyboardInput.keyChangedHandler =
        ^(GCKeyboardInput *input, GCControllerButtonInput *key, GCKeyCode keyCode, BOOL pressed) {
        (void)key;
        NCLRRunOnMain(^{
            NCLRHardwareInput *strongSelf = weakSelf;
            if (!strongSelf) return;
            if (keyCode == GCKeyCodeCapsLock && pressed) {
                strongSelf.capsLock = !strongSelf.capsLock;
            }
            uint8_t modifiers = [strongSelf modifiersForInput:input];
            NSNumber *code = @(keyCode);
            BOOL repeat = pressed && [strongSelf.keysDown containsObject:code];
            if (pressed) {
                [strongSelf.keysDown addObject:code];
            } else {
                [strongSelf.keysDown removeObject:code];
            }
            horizon_ios_hardware_key((uint16_t)keyCode, pressed ? 1 : 0, repeat ? 1 : 0, modifiers);
        });
    };
}

- (void)detachKeyboard:(BOOL)releaseKeys {
    GCKeyboard *keyboard = self.keyboard;
    if (keyboard) {
        keyboard.keyboardInput.keyChangedHandler = nil;
    }
    self.keyboard = nil;
    if (releaseKeys) {
        for (NSNumber *code in self.keysDown) {
            horizon_ios_hardware_key((uint16_t)code.unsignedIntegerValue, 0, 0, 0);
        }
    }
    [self.keysDown removeAllObjects];
    self.capsLock = NO;
}

- (void)attachMouse:(GCMouse *)mouse {
    if (!mouse) return;
    __weak typeof(self) weakSelf = self;
    mouse.mouseInput.mouseMovedHandler = ^(GCMouseInput *input, float deltaX, float deltaY) {
        (void)input;
        NCLRRunOnMain(^{ [weakSelf moveMouseX:deltaX y:deltaY]; });
    };
    mouse.mouseInput.leftButton.pressedChangedHandler =
        ^(GCControllerButtonInput *button, float value, BOOL pressed) {
        (void)button; (void)value;
        NCLRRunOnMain(^{ horizon_ios_hardware_mouse_button(0, pressed ? 1 : 0); });
    };
    mouse.mouseInput.rightButton.pressedChangedHandler =
        ^(GCControllerButtonInput *button, float value, BOOL pressed) {
        (void)button; (void)value;
        NCLRRunOnMain(^{ horizon_ios_hardware_mouse_button(1, pressed ? 1 : 0); });
    };
    mouse.mouseInput.middleButton.pressedChangedHandler =
        ^(GCControllerButtonInput *button, float value, BOOL pressed) {
        (void)button; (void)value;
        NCLRRunOnMain(^{ horizon_ios_hardware_mouse_button(2, pressed ? 1 : 0); });
    };
    // The scroll wheel arrives as raw axes: vertical motion rides the first
    // axis with the sign flipped, horizontal motion the second. The game reads
    // a positive delta as a scroll up, so negate the vertical axis here.
    mouse.mouseInput.scroll.valueChangedHandler =
        ^(GCControllerDirectionPad *pad, float xValue, float yValue) {
        (void)pad;
        float horizontal = yValue;
        float vertical = -xValue;
        if (horizontal == 0.0f && vertical == 0.0f) return;
        NCLRRunOnMain(^{ horizon_ios_hardware_mouse_scroll(horizontal, vertical); });
    };
    self.cursorReady = NO;
    [self moveMouseX:0 y:0];
}

- (void)detachMouse {
    GCMouse *mouse = self.mouse;
    if (mouse) {
        mouse.mouseInput.mouseMovedHandler = nil;
        mouse.mouseInput.leftButton.pressedChangedHandler = nil;
        mouse.mouseInput.rightButton.pressedChangedHandler = nil;
        mouse.mouseInput.middleButton.pressedChangedHandler = nil;
        mouse.mouseInput.scroll.valueChangedHandler = nil;
    }
    self.mouse = nil;
    for (int button = 0; button < 3; button++) {
        horizon_ios_hardware_mouse_button(button, 0);
    }
    self.cursorReady = NO;
}

- (void)moveMouseX:(float)deltaX y:(float)deltaY {
    UIWindow *window = UIApplication.sharedApplication.keyWindow;
    if (!window) return;
    CGSize size = window.bounds.size;
    if (size.width < 1.0 || size.height < 1.0) return;
    if (!self.cursorReady || !CGSizeEqualToSize(self.cursorBounds, size)) {
        self.cursor = CGPointMake(size.width * 0.5, size.height * 0.5);
        self.cursorBounds = size;
        self.cursorReady = YES;
    }
    CGFloat x = fmax(0.0, fmin(size.width - 1.0, self.cursor.x + deltaX));
    CGFloat y = fmax(0.0, fmin(size.height - 1.0, self.cursor.y + deltaY));
    self.cursor = CGPointMake(x, y);
    CGFloat scale = window.screen.scale > 0.0 ? window.screen.scale : UIScreen.mainScreen.scale;
    NCLRRunOnMain(^{
        // Relative motion stays in points so mouse look matches the cursor on
        // every display scale. The pointer position is a drawable pixel.
        horizon_ios_hardware_mouse_motion(deltaX, deltaY, x * scale, y * scale);
    });
}
@end

static NSString *NCLRModSwitchesPath(void) {
    return [NCLRDocuments() stringByAppendingPathComponent:@"horizon-mods.txt"];
}

static NSDictionary<NSString *, NSNumber *> *NCLRReadModSwitches(void) {
    NSString *text = [NSString stringWithContentsOfFile:NCLRModSwitchesPath()
                                               encoding:NSUTF8StringEncoding error:nil];
    NSMutableDictionary *switches = [NSMutableDictionary dictionary];
    for (NSString *line in [text componentsSeparatedByString:@"\n"]) {
        if (![line hasPrefix:@"mod."]) continue;
        NSRange colon = [line rangeOfString:@":"];
        if (colon.location == NSNotFound) continue;
        NSString *key = [line substringWithRange:NSMakeRange(3, colon.location - 3)];
        NSString *value = [[line substringFromIndex:colon.location + 1]
            stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceCharacterSet];
        switches[key] = @([value isEqualToString:@"true"]);
    }
    return switches;
}

static void NCLRWriteModSwitch(NSString *identifier, BOOL enabled, BOOL remove) {
    if (identifier.length == 0) return;
    NSString *path = NCLRModSwitchesPath();
    NSString *text = [NSString stringWithContentsOfFile:path encoding:NSUTF8StringEncoding error:nil] ?: @"";
    NSMutableArray<NSString *> *lines = [NSMutableArray array];
    NSString *prefix = [NSString stringWithFormat:@"mod.%@:", identifier];
    for (NSString *line in [text componentsSeparatedByString:@"\n"]) {
        if (line.length == 0 || [line hasPrefix:prefix]) continue;
        [lines addObject:line];
    }
    if (!remove) [lines addObject:[NSString stringWithFormat:@"mod.%@:%@", identifier, enabled ? @"true" : @"false"]];
    [[lines componentsJoinedByString:@"\n"] writeToFile:path atomically:YES
        encoding:NSUTF8StringEncoding error:nil];
}

@interface NCLRMod : NSObject
@property (nonatomic, copy) NSString *name;
@property (nonatomic, copy) NSString *path;
@property (nonatomic, assign) unsigned long long size;
@property (nonatomic, assign) BOOL enabled;
@property (nonatomic, copy) NSString *identifier;
@end

@implementation NCLRMod
@end

#pragma mark - Keymap

static NSString *const NCLRInventoryEscape = @"inventoryEscape";
static NSString *const NCLRPauseEscape = @"pauseEscape";
static NSString *const NCLRInventoryKeyboard = @"inventoryKeyboard";
static NSString *const NCLRPauseKeyboard = @"pauseKeyboard";

static BOOL NCLRIsMenuKey(NSString *key) {
    return [key isEqualToString:NCLRInventoryEscape] || [key isEqualToString:NCLRPauseEscape] ||
        [key isEqualToString:NCLRInventoryKeyboard] || [key isEqualToString:NCLRPauseKeyboard];
}

static CGFloat NCLRKeyWidthForPosition(NSDictionary *position, NSString *key) {
    CGFloat height = [position[@"size"] doubleValue];
    CGFloat width = [position[@"width"] doubleValue];
    if (!isfinite(width) || width <= 0.0) {
        width = NCLRIsMenuKey(key) ? height * 2.2 : height;
    }
    return MAX(0.045, MIN(1.0, width));
}

@interface NCLRKeymap : NSObject
@property (nonatomic, strong) NSMutableDictionary<NSString *, NSString *> *bindings;
@property (nonatomic, strong) NSMutableDictionary<NSString *, NSDictionary *> *positions;
@property (nonatomic, strong) NSMutableSet<NSString *> *deletedKeys;
@property (nonatomic, strong) NSMutableDictionary<NSString *, NSString *> *triggerModes;
@property (nonatomic, assign) CGFloat buttonSize;
@property (nonatomic, assign) CGFloat opacity;
@property (nonatomic, copy) NSString *touchMode;
@property (nonatomic, assign) BOOL dynamicJoystick;
@property (nonatomic, assign) NSInteger layoutVersion;
@property (nonatomic, assign) BOOL leftHanded;
@property (nonatomic, assign) BOOL joystickSprint;
@property (nonatomic, assign) CGFloat sensitivity;
@property (nonatomic, assign) BOOL gyroEnabled;
@property (nonatomic, assign) CGFloat gyroSensitivity;
@property (nonatomic, assign) BOOL gyroInvertX;
@property (nonatomic, assign) BOOL gyroInvertY;
@property (nonatomic, assign) CGFloat joystickDeadzone;
@property (nonatomic, assign) BOOL sprintButton;
@property (nonatomic, assign) NSInteger buttonStyle;
@property (nonatomic, assign) BOOL iconLabels;
@property (nonatomic, strong) NSMutableSet<NSString *> *hiddenKeys;
@property (nonatomic, strong) NSMutableArray<NSMutableDictionary *> *menuKeys;
+ (instancetype)shared;
- (BOOL)isHidden:(NSString *)key;
- (void)setHidden:(BOOL)hidden forKey:(NSString *)key;
- (void)applyPreset:(NSString *)preset;
- (void)load;
- (void)save;
- (NSString *)path;
- (void)addKey:(NSString *)key label:(NSString *)label;
- (NSString *)addMenuKey:(NSString *)key label:(NSString *)label screen:(NSInteger)screen;
- (NSString *)labelForCanvasKey:(NSString *)key;
- (BOOL)isMenuCanvasKey:(NSString *)key;
- (void)resetLayout;
@end

@implementation NCLRKeymap
+ (instancetype)shared {
    static NCLRKeymap *keymap;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        keymap = [[NCLRKeymap alloc] init];
        [keymap load];
    });
    return keymap;
}

- (NSString *)path {
    return [NCLRDocuments() stringByAppendingPathComponent:@"keymap.json"];
}

- (void)load {
    self.buttonSize = 1.0;
    self.bindings = [@{
        @"forward" : @"W",
        @"back" : @"S",
        @"left" : @"A",
        @"right" : @"D",
        @"jump" : @"Space",
        @"sneak" : @"Shift",
        @"sprint" : @"Ctrl",
        @"attack" : @"MouseLeft",
        @"use" : @"MouseRight",
        @"inventory" : @"E",
        @"chat" : @"T",
        @"pause" : @"Esc",
        @"settings" : @"F1",
        @"debug" : @"F3",
        @"perspective" : @"F5",
        @"offhand" : @"F",
        @"shield" : @"MouseRight",
        @"move" : @"移动",
        NCLRInventoryEscape : @"ESC",
        NCLRPauseEscape : @"ESC",
        NCLRInventoryKeyboard : @"键盘",
        NCLRPauseKeyboard : @"键盘",
    } mutableCopy];
    self.opacity = 0.55;
    self.layoutVersion = 2;
    self.deletedKeys = [NSMutableSet set];
    self.menuKeys = [NSMutableArray array];
    self.leftHanded = NO;
    self.joystickSprint = NO;
    self.sensitivity = 1.0;
    self.gyroEnabled = NO;
    self.gyroSensitivity = 1.0;
    self.gyroInvertX = NO;
    self.gyroInvertY = NO;
    self.joystickDeadzone = 0.16;
    self.sprintButton = YES;
    self.buttonStyle = 0;
    self.iconLabels = NO;
    self.hiddenKeys = [NSMutableSet setWithArray:@[@"settings", @"debug", @"perspective", @"offhand", @"shield", @"keyboard"]];
    self.touchMode = @"crosshair";
    self.dynamicJoystick = NO;
    self.positions = [@{
        NCLRInventoryEscape : @{@"x": @0.057, @"y": @0.066, @"size": @0.092},
        NCLRPauseEscape : @{@"x": @0.057, @"y": @0.066, @"size": @0.092},
        NCLRInventoryKeyboard : @{@"x": @0.18, @"y": @0.066, @"size": @0.092},
        NCLRPauseKeyboard : @{@"x": @0.18, @"y": @0.066, @"size": @0.092},
        @"forward" : @{ @"x" : @0.10, @"y" : @0.80, @"size" : @0.10 },
        @"back" : @{ @"x" : @0.16, @"y" : @0.88, @"size" : @0.08 },
        @"left" : @{ @"x" : @0.07, @"y" : @0.88, @"size" : @0.08 },
        @"right" : @{ @"x" : @0.20, @"y" : @0.74, @"size" : @0.08 },
        @"jump" : @{ @"x" : @0.86, @"y" : @0.78, @"size" : @0.15 },
        @"sneak" : @{ @"x" : @0.72, @"y" : @0.88, @"size" : @0.09 },
        @"sprint" : @{ @"x" : @0.30, @"y" : @0.90, @"size" : @0.07 },
        @"attack" : @{ @"x" : @0.93, @"y" : @0.62, @"size" : @0.13 },
        @"use" : @{ @"x" : @0.78, @"y" : @0.64, @"size" : @0.12 },
        @"inventory" : @{ @"x" : @0.76, @"y" : @0.12, @"size" : @0.065 },
        @"chat" : @{ @"x" : @0.84, @"y" : @0.12, @"size" : @0.065 },
        @"pause" : @{ @"x" : @0.92, @"y" : @0.12, @"size" : @0.065 },
        @"settings" : @{ @"x" : @0.68, @"y" : @0.12, @"size" : @0.060 },
        @"debug" : @{ @"x" : @0.60, @"y" : @0.12, @"size" : @0.055 },
        @"perspective" : @{ @"x" : @0.52, @"y" : @0.12, @"size" : @0.055 },
        @"offhand" : @{ @"x" : @0.44, @"y" : @0.12, @"size" : @0.060 },
        @"shield" : @{ @"x" : @0.36, @"y" : @0.12, @"size" : @0.060 },
        @"keyboard" : @{ @"x" : @0.66, @"y" : @0.12, @"size" : @0.065 },
        @"move" : @{ @"x" : @0.14, @"y" : @0.74, @"size" : @0.28 },
    } mutableCopy];
    NSData *data = [NSData dataWithContentsOfFile:self.path];
    if (data.length == 0) {
        [self save];
        return;
    }
    NSDictionary *json = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    if (![json isKindOfClass:[NSDictionary class]]) {
        return;
    }
    NSNumber *size = json[@"buttonSize"];
    if ([size isKindOfClass:[NSNumber class]]) {
        self.buttonSize = MAX(0.5, MIN(2.0, size.doubleValue));
    }
    NSNumber *opacity = json[@"opacity"];
    if ([opacity isKindOfClass:[NSNumber class]]) {
        self.opacity = MAX(0.15, MIN(1.0, opacity.doubleValue));
    }
    NSString *mode = NCLRString(json[@"touchMode"]);
    if (mode.length > 0) {
        self.touchMode = mode;
    }
    NSNumber *dynamic = json[@"dynamicJoystick"];
    if ([dynamic isKindOfClass:[NSNumber class]]) {
        self.dynamicJoystick = dynamic.boolValue;
    }
    NSNumber *left = json[@"leftHanded"];
    if ([left isKindOfClass:[NSNumber class]]) self.leftHanded = left.boolValue;
    NSNumber *joystickSprint = json[@"joystickSprint"];
    if ([joystickSprint isKindOfClass:[NSNumber class]]) self.joystickSprint = joystickSprint.boolValue;
    NSNumber *gyroEnabled = json[@"gyroEnabled"];
    if ([gyroEnabled isKindOfClass:NSNumber.class]) self.gyroEnabled = gyroEnabled.boolValue;
    NSNumber *gyroInvertX = json[@"gyroInvertX"];
    NSNumber *gyroInvertY = json[@"gyroInvertY"];
    if ([gyroInvertX isKindOfClass:NSNumber.class]) self.gyroInvertX = gyroInvertX.boolValue;
    if ([gyroInvertY isKindOfClass:NSNumber.class]) self.gyroInvertY = gyroInvertY.boolValue;
    NSNumber *gyroSensitivity = json[@"gyroSensitivity"];
    if ([gyroSensitivity isKindOfClass:NSNumber.class] && isfinite(gyroSensitivity.doubleValue)) {
        self.gyroSensitivity = MAX(0.25, MIN(2.5, gyroSensitivity.doubleValue));
    }
    NSNumber *sensitivity = json[@"sensitivity"];
    if ([sensitivity isKindOfClass:[NSNumber class]]) {
        self.sensitivity = MAX(0.3, MIN(2.5, sensitivity.doubleValue));
    }
    if ([json[@"triggerModes"] isKindOfClass:NSDictionary.class]) {
        self.triggerModes = [json[@"triggerModes"] mutableCopy];
    }
    NSNumber *deadzone = json[@"joystickDeadzone"];
    if ([deadzone isKindOfClass:[NSNumber class]]) {
        self.joystickDeadzone = MAX(0.05, MIN(0.35, deadzone.doubleValue));
    }
    NSNumber *sprintButton = json[@"sprintButton"];
    if ([sprintButton isKindOfClass:[NSNumber class]]) {
        self.sprintButton = sprintButton.boolValue;
    }
    NSNumber *buttonStyle = json[@"buttonStyle"];
    if ([buttonStyle isKindOfClass:[NSNumber class]]) {
        self.buttonStyle = MAX(0, MIN(2, buttonStyle.integerValue));
    }
    NSNumber *iconLabels = json[@"iconLabels"];
    if ([iconLabels isKindOfClass:[NSNumber class]]) {
        self.iconLabels = iconLabels.boolValue;
    }
    NSDictionary *saved = json[@"bindings"];
    if ([saved isKindOfClass:[NSDictionary class]]) {
        for (NSString *key in saved) {
            NSString *value = NCLRString(saved[key]);
            if (key.length > 0 && value.length > 0) {
                self.bindings[key] = value;
            }
        }
    }
    NSDictionary *layout = json[@"layout"];
    if ([layout isKindOfClass:[NSDictionary class]]) {
        for (NSString *key in layout) {
            id value = layout[key];
            if (![value isKindOfClass:NSDictionary.class]) continue;
            NSNumber *x = value[@"x"], *y = value[@"y"], *size = value[@"size"], *width = value[@"width"];
            if (![x isKindOfClass:NSNumber.class] || ![y isKindOfClass:NSNumber.class] ||
                !isfinite(x.doubleValue) || !isfinite(y.doubleValue) ||
                x.doubleValue < 0 || x.doubleValue > 1 || y.doubleValue < 0 || y.doubleValue > 1) continue;
            if (![size isKindOfClass:NSNumber.class] || !isfinite(size.doubleValue)) {
                size = self.positions[key][@"size"] ?: @0.12;
            }
            CGFloat widthValue = [width isKindOfClass:NSNumber.class] && isfinite(width.doubleValue)
                ? width.doubleValue : (NCLRIsMenuKey(key) ? size.doubleValue * 2.2 : size.doubleValue);
            self.positions[key] = @{@"x": x, @"y": y,
                                    @"size": @(MAX(0.045, MIN(0.45, size.doubleValue))),
                                    @"width": @(MAX(0.045, MIN(1.0, widthValue)))};

        }
    }
    NSArray *menuKeys = json[@"menuKeys"];
    if ([menuKeys isKindOfClass:NSArray.class]) {
        [self.menuKeys removeAllObjects];
        for (id raw in menuKeys) {
            if (![raw isKindOfClass:NSDictionary.class]) continue;
            NSString *identifier = NCLRString(raw[@"id"]);
            NSString *screen = NCLRString(raw[@"screen"]);
            NSString *label = NCLRString(raw[@"label"]);
            BOOL validScreen = [screen isEqualToString:@"inventory"] || [screen isEqualToString:@"pause"];
            if (identifier.length == 0 || !validScreen ||
                ![label isKindOfClass:NSString.class] || label.length == 0) continue;
            [self.menuKeys addObject:[@{@"id": identifier, @"screen": screen, @"label": label} mutableCopy]];
            if (self.positions[identifier] == nil) {
                self.positions[identifier] = @{@"x": @0.5, @"y": @0.5,
                                               @"size": @0.12, @"width": @0.12};
            }
        }
    }
    NSArray *deleted = json[@"deletedKeys"];
    if ([deleted isKindOfClass:NSArray.class]) {
        for (id key in deleted) {
            if ([key isKindOfClass:NSString.class]) [self.deletedKeys addObject:key];
        }
    }
    NSArray *hidden = json[@"hidden"];
    if ([hidden isKindOfClass:[NSArray class]]) {
        [self.hiddenKeys removeAllObjects];
        for (NSString *key in hidden) {
            if ([key isKindOfClass:[NSString class]]) [self.hiddenKeys addObject:key];
        }
    }
    if (self.bindings[@"move"] == nil) {
        self.bindings[@"move"] = @"移动";
    }
    if (self.positions[@"move"] == nil) {
        self.positions[@"move"] = @{ @"x" : @0.16, @"y" : @0.78, @"size" : @0.30 };
    }
}

- (void)addKey:(NSString *)key label:(NSString *)label {
    if (key.length == 0) {
        return;
    }
    if ([key isEqualToString:@"keyboard"]) {
        // The keyboard control ships hidden by default; adding it from the
        // picker means making that control visible and placing it.
        self.bindings[key] = label.length > 0 ? label : @"系统键盘";
        [self.deletedKeys removeObject:key];
        [self.hiddenKeys removeObject:key];
        if (self.positions[key] == nil) {
            self.positions[key] = @{ @"x" : @0.50, @"y" : @0.50,
                                      @"size" : @0.075, @"width" : @0.075 };
        }
        return;
    }
    if ([self.deletedKeys containsObject:key]) {
        [self.deletedKeys removeObject:key];
        return;
    }
    if (self.bindings[key] != nil) {
        return;
    }
    self.bindings[key] = label.length > 0 ? label : key;
    self.positions[key] = @{ @"x" : @0.50, @"y" : @0.50, @"size" : @0.12, @"width" : @0.12 };
}

- (NSString *)addMenuKey:(NSString *)key label:(NSString *)label screen:(NSInteger)screen {
    NSString *screenName = screen == 1 ? @"inventory" : @"pause";
    NSString *identifier = [NSString stringWithFormat:@"menu.%@.%@",
                            screenName, NSUUID.UUID.UUIDString.lowercaseString];
    NSDictionary *item = @{@"id": identifier, @"screen": screenName,
                           @"label": label.length > 0 ? label : key,
                           @"x": @0.50, @"y": @0.30,
                           @"size": @0.12, @"width": @0.12};
    [self.menuKeys addObject:[item mutableCopy]];
    self.positions[identifier] = @{@"x": @0.50, @"y": @0.30,
                                   @"size": @0.12, @"width": @0.12};
    return identifier;
}

- (BOOL)isMenuCanvasKey:(NSString *)key {
    for (NSDictionary *item in self.menuKeys) {
        if ([item[@"id"] isEqual:key]) return YES;
    }
    return NO;
}

- (NSString *)labelForCanvasKey:(NSString *)key {
    for (NSDictionary *item in self.menuKeys) {
        if ([item[@"id"] isEqual:key]) return item[@"label"];
    }
    return self.bindings[key] ?: key;
}

- (void)resetLayout {
    [self.deletedKeys removeAllObjects];
    [self.menuKeys removeAllObjects];
    self.positions = [@{
        NCLRInventoryEscape : @{@"x": @0.057, @"y": @0.066, @"size": @0.092},
        NCLRPauseEscape : @{@"x": @0.057, @"y": @0.066, @"size": @0.092},
        NCLRInventoryKeyboard : @{@"x": @0.18, @"y": @0.066, @"size": @0.092},
        NCLRPauseKeyboard : @{@"x": @0.18, @"y": @0.066, @"size": @0.092},
        @"forward" : @{ @"x" : @0.10, @"y" : @0.80, @"size" : @0.10 },
        @"back" : @{ @"x" : @0.16, @"y" : @0.88, @"size" : @0.08 },
        @"left" : @{ @"x" : @0.07, @"y" : @0.88, @"size" : @0.08 },
        @"right" : @{ @"x" : @0.20, @"y" : @0.74, @"size" : @0.08 },
        @"jump" : @{ @"x" : @0.86, @"y" : @0.78, @"size" : @0.15 },
        @"sneak" : @{ @"x" : @0.72, @"y" : @0.88, @"size" : @0.09 },
        @"sprint" : @{ @"x" : @0.30, @"y" : @0.90, @"size" : @0.07 },
        @"attack" : @{ @"x" : @0.93, @"y" : @0.62, @"size" : @0.13 },
        @"use" : @{ @"x" : @0.78, @"y" : @0.64, @"size" : @0.12 },
        @"inventory" : @{ @"x" : @0.76, @"y" : @0.12, @"size" : @0.065 },
        @"chat" : @{ @"x" : @0.84, @"y" : @0.12, @"size" : @0.065 },
        @"pause" : @{ @"x" : @0.92, @"y" : @0.12, @"size" : @0.065 },
        @"settings" : @{ @"x" : @0.68, @"y" : @0.12, @"size" : @0.060 },
        @"debug" : @{ @"x" : @0.60, @"y" : @0.12, @"size" : @0.055 },
        @"perspective" : @{ @"x" : @0.52, @"y" : @0.12, @"size" : @0.055 },
        @"offhand" : @{ @"x" : @0.44, @"y" : @0.12, @"size" : @0.060 },
        @"shield" : @{ @"x" : @0.36, @"y" : @0.12, @"size" : @0.060 },
        @"keyboard" : @{ @"x" : @0.66, @"y" : @0.12, @"size" : @0.065 },
        @"move" : @{ @"x" : @0.14, @"y" : @0.74, @"size" : @0.28 },
    } mutableCopy];
    [self.hiddenKeys setSet:[NSSet setWithArray:@[@"settings", @"debug", @"perspective", @"offhand", @"shield", @"keyboard"]]];
}

- (BOOL)isHidden:(NSString *)key {
    return [self.hiddenKeys containsObject:key];
}

- (void)setHidden:(BOOL)hidden forKey:(NSString *)key {
    if (key.length == 0) return;
    if (hidden) {
        [self.hiddenKeys addObject:key];
    } else {
        [self.hiddenKeys removeObject:key];
    }
}

- (void)applyPreset:(NSString *)preset {
    NSDictionary *inventoryEscape = self.positions[NCLRInventoryEscape];
    NSDictionary *pauseEscape = self.positions[NCLRPauseEscape];
    NSDictionary *inventoryKeyboard = self.positions[NCLRInventoryKeyboard];
    NSDictionary *pauseKeyboard = self.positions[NCLRPauseKeyboard];
    if ([preset isEqualToString:@"classic"]) {
        self.positions = [@{
            @"forward" : @{ @"x" : @0.13, @"y" : @0.76, @"size" : @0.16 },
            @"back" : @{ @"x" : @0.24, @"y" : @0.86, @"size" : @0.10 },
            @"left" : @{ @"x" : @0.08, @"y" : @0.86, @"size" : @0.10 },
            @"right" : @{ @"x" : @0.24, @"y" : @0.66, @"size" : @0.10 },
            @"jump" : @{ @"x" : @0.92, @"y" : @0.78, @"size" : @0.16 },
            @"sneak" : @{ @"x" : @0.81, @"y" : @0.88, @"size" : @0.10 },
            @"sprint" : @{ @"x" : @0.72, @"y" : @0.78, @"size" : @0.10 },
            @"attack" : @{ @"x" : @0.80, @"y" : @0.62, @"size" : @0.16 },
            @"use" : @{ @"x" : @0.92, @"y" : @0.48, @"size" : @0.16 },
            @"inventory" : @{ @"x" : @0.72, @"y" : @0.95, @"size" : @0.08 },
            @"chat" : @{ @"x" : @0.84, @"y" : @0.12, @"size" : @0.065 },
            @"pause" : @{ @"x" : @0.92, @"y" : @0.12, @"size" : @0.065 },
            @"settings" : @{ @"x" : @0.68, @"y" : @0.12, @"size" : @0.060 },
            @"debug" : @{ @"x" : @0.60, @"y" : @0.12, @"size" : @0.055 },
            @"perspective" : @{ @"x" : @0.52, @"y" : @0.12, @"size" : @0.055 },
            @"offhand" : @{ @"x" : @0.44, @"y" : @0.12, @"size" : @0.060 },
            @"shield" : @{ @"x" : @0.36, @"y" : @0.12, @"size" : @0.060 },
            @"keyboard" : @{ @"x" : @0.66, @"y" : @0.12, @"size" : @0.065 },
            @"move" : @{ @"x" : @0.16, @"y" : @0.78, @"size" : @0.30 },
        } mutableCopy];
        [self.hiddenKeys removeAllObjects];
    } else {
        [self resetLayout];
        if ([preset isEqualToString:@"minimal"]) {
            [self.hiddenKeys addObjectsFromArray:@[@"chat", @"inventory"]];
        }
    }
    if (inventoryEscape) self.positions[NCLRInventoryEscape] = inventoryEscape;
    if (pauseEscape) self.positions[NCLRPauseEscape] = pauseEscape;
    if (inventoryKeyboard) self.positions[NCLRInventoryKeyboard] = inventoryKeyboard;
    if (pauseKeyboard) self.positions[NCLRPauseKeyboard] = pauseKeyboard;
}

- (void)save {
    NSDictionary *json = @{
        @"layoutVersion" : @(self.layoutVersion),
        @"buttonSize" : @(self.buttonSize),
        @"opacity" : @(self.opacity),
        @"touchMode" : self.touchMode ?: @"crosshair",
        @"dynamicJoystick" : @(self.dynamicJoystick),
        @"leftHanded" : @(self.leftHanded),
        @"joystickSprint" : @(self.joystickSprint),
        @"sensitivity" : @(self.sensitivity),
        @"gyroEnabled" : @(self.gyroEnabled),
        @"gyroSensitivity" : @(self.gyroSensitivity),
        @"gyroInvertX" : @(self.gyroInvertX),
        @"gyroInvertY" : @(self.gyroInvertY),
        @"joystickDeadzone" : @(self.joystickDeadzone),
        @"sprintButton" : @(self.sprintButton),
        @"buttonStyle" : @(self.buttonStyle),
        @"iconLabels" : @(self.iconLabels),
        @"hidden" : self.hiddenKeys.allObjects ?: @[],
        @"deletedKeys" : self.deletedKeys.allObjects ?: @[],
        @"menuKeys" : self.menuKeys ?: @[],
        @"bindings" : self.bindings ?: @{},
        @"layout" : self.positions ?: @{},
        @"triggerModes" : self.triggerModes ?: @{},
    };
    NSData *data = [NSJSONSerialization dataWithJSONObject:json
                                                   options:NSJSONWritingPrettyPrinted
                                                     error:nil];
    [data writeToFile:self.path atomically:YES];
}
@end

#pragma mark - Store

@interface NCLRStore : NSObject
@property (nonatomic, strong) NSMutableArray<NCLRVersion *> *versions;
@property (nonatomic, strong) NSMutableArray<NCLRMod *> *mods;
@property (nonatomic, copy) NSString *selectedVersion;
@property (nonatomic, copy) NSString *clientId;
+ (instancetype)shared;
- (void)save;
- (NCLRVersion *)selected;
- (void)selectVersion:(NCLRVersion *)version;
- (NCLRVersion *)addVersionNamed:(NSString *)name game:(NSString *)game loader:(NSString *)loader;
- (void)deleteVersion:(NCLRVersion *)version;
- (void)reloadMods;
- (void)migrateLegacyMods;
- (BOOL)importModAtURL:(NSURL *)url error:(NSString **)error;
- (void)deleteMod:(NCLRMod *)mod;
- (void)setMod:(NCLRMod *)mod enabled:(BOOL)enabled;
- (NSString *)versionsDirectory;
- (NSString *)modsDirectory;
- (NSString *)configPath;
@end

@implementation NCLRStore
+ (instancetype)shared {
    static NCLRStore *store;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ store = [[NCLRStore alloc] init]; });
    return store;
}

- (instancetype)init {
    self = [super init];
    if (self) {
        _versions = [NSMutableArray array];
        _mods = [NSMutableArray array];
        [self load];
    }
    return self;
}

- (NSString *)versionsDirectory {
    NSString *path = [NCLRDocuments() stringByAppendingPathComponent:@"versions"];
    NCLRCreateDirectory(path);
    return path;
}

- (NSString *)modsDirectory {
    // Horizon reads content mods from `horizon-mods`, not the old `mods`
    // folder. Keep the launcher's import destination in sync with the engine.
    NSString *path = [NCLRDocuments() stringByAppendingPathComponent:@"horizon-mods"];
    NCLRCreateDirectory(path);
    return path;
}

- (NSString *)configPath {
    return [NCLRDocuments() stringByAppendingPathComponent:@"nclr-launcher.json"];
}

- (void)load {
    NSString *savedClientId = [[NSUserDefaults standardUserDefaults] stringForKey:kNCLRClientKey];
    self.clientId = savedClientId.length > 0 ? savedClientId : kNCLRDefaultClientID;
    NSData *data = [NSData dataWithContentsOfFile:self.configPath];
    NSDictionary *json = nil;
    if (data.length > 0) {
        json = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    }
    if ([json isKindOfClass:[NSDictionary class]]) {
        for (NSDictionary *item in json[@"versions"] ?: @[]) {
            if ([item isKindOfClass:[NSDictionary class]]) {
                [self.versions addObject:[NCLRVersion fromDictionary:item]];
            }
        }
        self.selectedVersion = NCLRString(json[@"selectedVersion"]) ?: @"";
    }
    if (self.versions.count == 0) {
        NCLRVersion *version = [[NCLRVersion alloc] init];
        version.identifier = @"26.3";
        version.name = @"Minecraft 26.3";
        version.gameVersion = @"26.3";
        version.loader = @"Vanilla";
        version.installed = NO;
        [self.versions addObject:version];
        self.selectedVersion = version.identifier;
        [self save];
    }
    if (self.selectedVersion.length == 0) {
        self.selectedVersion = self.versions.firstObject.identifier;
    }
    [self migrateLegacyMods];
    [self reloadMods];
}

- (void)save {
    NSMutableArray *items = [NSMutableArray array];
    for (NCLRVersion *version in self.versions) {
        [items addObject:[version dictionary]];
    }
    NSDictionary *json = @{
        @"versions" : items,
        @"selectedVersion" : self.selectedVersion ?: @"",
    };
    NSData *data = [NSJSONSerialization dataWithJSONObject:json options:NSJSONWritingPrettyPrinted error:nil];
    [data writeToFile:self.configPath atomically:YES];
    [[NSUserDefaults standardUserDefaults] setObject:self.clientId ?: @"" forKey:kNCLRClientKey];
}

- (NCLRVersion *)selected {
    for (NCLRVersion *version in self.versions) {
        if ([version.identifier isEqualToString:self.selectedVersion]) {
            return version;
        }
    }
    return self.versions.firstObject;
}

- (void)selectVersion:(NCLRVersion *)version {
    self.selectedVersion = version.identifier;
    [self save];
}

- (NCLRVersion *)addVersionNamed:(NSString *)name game:(NSString *)game loader:(NSString *)loader {
    NSString *base = name.length > 0 ? name : game;
    NSString *identifier = [[base lowercaseString]
        stringByReplacingOccurrencesOfString:@" " withString:@"-"];
    identifier = [[identifier componentsSeparatedByCharactersInSet:
        [[NSCharacterSet alphanumericCharacterSet] invertedSet]] componentsJoinedByString:@"-"];
    if (identifier.length == 0) {
        identifier = [NSString stringWithFormat:@"version-%lu", (unsigned long)self.versions.count + 1];
    }
    NCLRVersion *version = [[NCLRVersion alloc] init];
    version.identifier = identifier;
    version.name = name.length > 0 ? name : [NSString stringWithFormat:@"Minecraft %@", game];
    version.gameVersion = game.length > 0 ? game : @"26.3";
    version.loader = loader.length > 0 ? loader : @"Vanilla";
    version.installed = NO;
    [self.versions addObject:version];
    NCLRCreateDirectory([[self versionsDirectory] stringByAppendingPathComponent:identifier]);
    [self save];
    return version;
}

- (void)deleteVersion:(NCLRVersion *)version {
    [self.versions removeObject:version];
    if ([self.selectedVersion isEqualToString:version.identifier]) {
        self.selectedVersion = self.versions.firstObject.identifier ?: @"";
    }
    [self save];
}

- (void)migrateLegacyMods {
    NSString *legacy = [NCLRDocuments() stringByAppendingPathComponent:@"mods"];
    NSArray<NSString *> *files = [[NSFileManager defaultManager]
        contentsOfDirectoryAtPath:legacy error:nil];
    for (NSString *file in files) {
        if (![file.pathExtension.lowercaseString isEqualToString:@"hmod"]) {
            continue;
        }
        NSString *destination = [self.modsDirectory stringByAppendingPathComponent:file];
        if (![[NSFileManager defaultManager] fileExistsAtPath:destination]) {
            [[NSFileManager defaultManager] copyItemAtPath:[legacy stringByAppendingPathComponent:file]
                                                    toPath:destination error:nil];
        }
    }
}

- (void)reloadMods {
    [self.mods removeAllObjects];
    NSArray<NSString *> *files = [[NSFileManager defaultManager]
        contentsOfDirectoryAtPath:self.modsDirectory error:nil];
    NSDictionary *enabled = [[NSUserDefaults standardUserDefaults] dictionaryForKey:@"NCLRModsEnabled"] ?: @{};
    NSDictionary *switches = NCLRReadModSwitches();
    for (NSString *file in files) {
        NSString *extension = file.pathExtension.lowercaseString;
        if (![extension isEqualToString:@"hmod"]) {
            continue;
        }
        NSString *path = [self.modsDirectory stringByAppendingPathComponent:file];
        NSDictionary *attributes = [[NSFileManager defaultManager] attributesOfItemAtPath:path error:nil];
        NCLRMod *mod = [[NCLRMod alloc] init];
        mod.name = file;
        mod.path = path;
        mod.size = [attributes[NSFileSize] unsignedLongLongValue];
        mod.identifier = NCLRModIdentifier(path) ?: file.stringByDeletingPathExtension;
        NSNumber *gameSwitch = switches[mod.identifier];
        mod.enabled = gameSwitch != nil ? gameSwitch.boolValue : [enabled[file] boolValue];
        [self.mods addObject:mod];
    }
    [self.mods sortUsingComparator:^NSComparisonResult(NCLRMod *a, NCLRMod *b) {
        return [a.name localizedCaseInsensitiveCompare:b.name];
    }];
}

- (BOOL)importModAtURL:(NSURL *)url error:(NSString **)error {
    if (![url.pathExtension.lowercaseString isEqualToString:@"hmod"]) {
        if (error) {
            *error = @"Horizon 只加载 .hmod 模组；Java .jar 需要先用 horizon-port-pack 移植。";
        }
        return NO;
    }
    BOOL scoped = [url startAccessingSecurityScopedResource];
    NSString *destination = [self.modsDirectory stringByAppendingPathComponent:url.lastPathComponent];
    [[NSFileManager defaultManager] removeItemAtPath:destination error:nil];
    NSError *copyError = nil;
    BOOL ok = [[NSFileManager defaultManager] copyItemAtURL:url toURL:[NSURL fileURLWithPath:destination]
                                                       error:&copyError];
    if (scoped) {
        [url stopAccessingSecurityScopedResource];
    }
    if (!ok && error) {
        *error = copyError.localizedDescription ?: @"无法导入模组";
    }
    if (ok) {
        [self reloadMods];
        for (NCLRMod *mod in self.mods) {
            if ([mod.name isEqualToString:url.lastPathComponent]) {
                [self setMod:mod enabled:YES];
                break;
            }
        }
    }
    return ok;
}

- (void)deleteMod:(NCLRMod *)mod {
    NCLRWriteModSwitch(mod.identifier, NO, YES);
    [[NSFileManager defaultManager] removeItemAtPath:mod.path error:nil];
    [self reloadMods];
}

- (void)setMod:(NCLRMod *)mod enabled:(BOOL)enabled {
    NSMutableDictionary *values = [[[NSUserDefaults standardUserDefaults]
        dictionaryForKey:@"NCLRModsEnabled"] mutableCopy] ?: [NSMutableDictionary dictionary];
    values[mod.name] = @(enabled);
    [[NSUserDefaults standardUserDefaults] setObject:values forKey:@"NCLRModsEnabled"];
    NCLRWriteModSwitch(mod.identifier, enabled, NO);
    mod.enabled = enabled;
}
@end

#pragma mark - Auth

static NSString *NCLRTextureCache(NSString *uuid, NSString *filename) {
    if (uuid.length != 32 ||
        [uuid rangeOfCharacterFromSet:[[NSCharacterSet characterSetWithCharactersInString:
            @"0123456789abcdefABCDEF"] invertedSet]].location != NSNotFound) return nil;
    NSString *directory = NSTemporaryDirectory();
    return [directory stringByAppendingPathComponent:[NSString stringWithFormat:@"nclr-%@-%@", uuid, filename]];
}

static NSDictionary *NCLRReadAppearance(NSString *uuid) {
    if (uuid.length == 0) return nil;
    NSData *data = [NSData dataWithContentsOfFile:[NCLRDocuments() stringByAppendingPathComponent:@".nclr-appearance.json"]];
    id profile = data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
    return [profile isKindOfClass:NSDictionary.class] && [profile[@"id"] isEqualToString:uuid] ? profile : nil;
}

static void NCLRSaveAppearance(NSDictionary *profile) {
    NSDictionary *appearance = @{
        @"id": profile[@"id"], @"skins": profile[@"skins"] ?: @[], @"capes": profile[@"capes"] ?: @[]
    };
    NSData *data = [NSJSONSerialization dataWithJSONObject:appearance options:0 error:nil];
    [data writeToFile:[NCLRDocuments() stringByAppendingPathComponent:@".nclr-appearance.json"] atomically:YES];
}

static NSDictionary *NCLRActiveTexture(NSDictionary *profile, NSString *key) {
    id values = profile[key];
    if (![values isKindOfClass:NSArray.class]) return nil;
    for (id entry in values) {
        if ([entry isKindOfClass:NSDictionary.class] && [entry[@"state"] isEqual:@"ACTIVE"]) return entry;
    }
    return nil;
}

static void NCLRFetchTexture(NSDictionary *entry, void (^completion)(UIImage *)) {
    NSString *source = NCLRString(entry[@"url"]);
    NSURLComponents *parts = source ? [NSURLComponents componentsWithString:source] : nil;
    if (![parts.host.lowercaseString isEqualToString:@"textures.minecraft.net"] ||
        ![parts.path hasPrefix:@"/texture/"] || parts.user || parts.password || parts.port) {
        completion(nil);
        return;
    }
    parts.scheme = @"https";
    NSURLRequest *request = [NSURLRequest requestWithURL:parts.URL
        cachePolicy:NSURLRequestReturnCacheDataElseLoad timeoutInterval:12];
    [[NSURLSession.sharedSession dataTaskWithRequest:request
        completionHandler:^(NSData *data, NSURLResponse *response, NSError *error) {
        UIImage *texture = !error && [(NSHTTPURLResponse *)response statusCode] == 200 && data.length < 1024 * 1024
            ? [UIImage imageWithData:data] : nil;
        dispatch_async(dispatch_get_main_queue(), ^{ completion(texture); });
    }] resume];
}

typedef void (^NCLRAuthCompletion)(NSString *token, NSString *name, NSString *error);

@interface NCLRAuth : NSObject <ASWebAuthenticationPresentationContextProviding>
+ (instancetype)shared;
- (void)startWithBrowserClientId:(NSString *)clientId
                       presenter:(UIViewController *)presenter
                      completion:(NCLRAuthCompletion)completion;
- (void)startWithClientId:(NSString *)clientId
                presenter:(UIViewController *)presenter
               completion:(NCLRAuthCompletion)completion;
- (void)logout;
- (void)fetchProfileWithToken:(NSString *)token completion:(NCLRAuthCompletion)completion;
- (void)refreshAppearance:(void (^)(NSDictionary *profile))completion;
@property (nonatomic, copy) NSString *token;
@property (nonatomic, copy) NSString *name;
@property (nonatomic, assign) BOOL developerPremium;
@property (nonatomic, copy) NSString *avatarURL;
@property (nonatomic, copy) NSString *uuid;
@property (nonatomic, strong) ASWebAuthenticationSession *session;
@property (nonatomic, weak) UIViewController *presenter;
@property (nonatomic, strong) SFSafariViewController *authSafari;
@end

@implementation NCLRAuth
+ (instancetype)shared {
    static NCLRAuth *auth;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ auth = [[NCLRAuth alloc] init]; });
    return auth;
}

- (instancetype)init {
    self = [super init];
    if (self) {
        _token = NCLRKeychainRead();
        _name = [[NSUserDefaults standardUserDefaults] stringForKey:kNCLRAccountNameKey];
        if (_token.length > 0) {
            _uuid = [[NSUserDefaults standardUserDefaults] stringForKey:kNCLRAccountUUIDKey];
            _avatarURL = NCLRAvatarURLForName(_name);
        }
        _developerPremium = NO;
    }
    return self;
}

- (void)logout {
    self.token = nil;
    self.name = nil;
    self.uuid = nil;
    self.avatarURL = nil;
    [[NSUserDefaults standardUserDefaults] removeObjectForKey:kNCLRAccountUUIDKey];
    NCLRKeychainWrite(nil);
    [[NSUserDefaults standardUserDefaults] removeObjectForKey:kNCLRAccountNameKey];
    [NSFileManager.defaultManager removeItemAtPath:
        [NCLRDocuments() stringByAppendingPathComponent:@".nclr-appearance.json"] error:nil];
    unsetenv("HORIZON_SESSION_FILE");
    [NSFileManager.defaultManager removeItemAtPath:
        [NCLRDocuments() stringByAppendingPathComponent:@".nclr-session.json"] error:nil];
}

- (void)fail:(NSString *)message presenter:(UIViewController *)presenter completion:(NCLRAuthCompletion)completion {
    dispatch_async(dispatch_get_main_queue(), ^{
        completion(nil, nil, message ?: @"登录失败");
    });
    (void)presenter;
}

- (void)postForm:(NSString *)urlString
           body:(NSString *)body
     completion:(void (^)(NSDictionary *json, NSString *error))completion {
    NSMutableURLRequest *request = [NSMutableURLRequest requestWithURL:[NSURL URLWithString:urlString]];
    request.HTTPMethod = @"POST";
    [request setValue:@"application/x-www-form-urlencoded" forHTTPHeaderField:@"Content-Type"];
    request.HTTPBody = [body dataUsingEncoding:NSUTF8StringEncoding];
    [[NSURLSession.sharedSession dataTaskWithRequest:request
        completionHandler:^(NSData *data, NSURLResponse *response, NSError *error) {
            if (error) {
                completion(nil, error.localizedDescription);
                return;
            }
            NSDictionary *json = data.length > 0
                ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
            if (![json isKindOfClass:[NSDictionary class]]) {
                completion(nil, @"登录服务器返回了无法解析的响应");
                return;
            }
            completion(json, nil);
        }] resume];
}

- (void)postJSON:(NSString *)urlString
            body:(NSDictionary *)body
         headers:(NSDictionary<NSString *, NSString *> *)headers
      completion:(void (^)(NSDictionary *json, NSInteger status, NSString *error))completion {
    NSMutableURLRequest *request = [NSMutableURLRequest requestWithURL:[NSURL URLWithString:urlString]];
    request.HTTPMethod = @"POST";
    [request setValue:@"application/json" forHTTPHeaderField:@"Content-Type"];
    [request setValue:@"application/json" forHTTPHeaderField:@"Accept"];
    for (NSString *key in headers) {
        [request setValue:headers[key] forHTTPHeaderField:key];
    }
    request.HTTPBody = [NSJSONSerialization dataWithJSONObject:body options:0 error:nil];
    [[NSURLSession.sharedSession dataTaskWithRequest:request
        completionHandler:^(NSData *data, NSURLResponse *response, NSError *error) {
            if (error) {
                completion(nil, 0, error.localizedDescription);
                return;
            }
            NSInteger status = [(NSHTTPURLResponse *)response statusCode];
            NSDictionary *json = data.length > 0
                ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
            if (![json isKindOfClass:[NSDictionary class]]) {
                json = @{};
            }
            completion(json, status, nil);
        }] resume];
}

- (ASPresentationAnchor)presentationAnchorForWebAuthenticationSession:
    (ASWebAuthenticationSession *)session {
    return self.presenter.view.window ?: UIWindow.new;
}

- (void)startWithBrowserClientId:(NSString *)clientId
                       presenter:(UIViewController *)presenter
                      completion:(NCLRAuthCompletion)completion {
    if (clientId.length == 0) {
        completion(nil, nil, @"未配置 Azure Client ID");
        return;
    }
    self.presenter = presenter;
    unsigned char random[48];
    arc4random_buf(random, sizeof(random));
    NSData *randomData = [NSData dataWithBytes:random length:sizeof(random)];
    NSString *verifier = [randomData base64EncodedStringWithOptions:0];
    verifier = [verifier stringByReplacingOccurrencesOfString:@"+" withString:@"-"];
    verifier = [verifier stringByReplacingOccurrencesOfString:@"/" withString:@"_"];
    verifier = [verifier stringByReplacingOccurrencesOfString:@"=" withString:@""];

    unsigned char digest[CC_SHA256_DIGEST_LENGTH];
    CC_SHA256(verifier.UTF8String, (CC_LONG)strlen(verifier.UTF8String), digest);
    NSData *digestData = [NSData dataWithBytes:digest length:sizeof(digest)];
    NSString *challenge = [digestData base64EncodedStringWithOptions:0];
    challenge = [challenge stringByReplacingOccurrencesOfString:@"+" withString:@"-"];
    challenge = [challenge stringByReplacingOccurrencesOfString:@"/" withString:@"_"];
    challenge = [challenge stringByReplacingOccurrencesOfString:@"=" withString:@""];

    NSURLComponents *components = [NSURLComponents componentsWithString:
        @"https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize"];
    components.queryItems = @[
        [NSURLQueryItem queryItemWithName:@"client_id" value:clientId],
        [NSURLQueryItem queryItemWithName:@"response_type" value:@"code"],
        [NSURLQueryItem queryItemWithName:@"redirect_uri" value:@"nclr://auth"],
        [NSURLQueryItem queryItemWithName:@"scope" value:@"offline_access XboxLive.signin"],
        [NSURLQueryItem queryItemWithName:@"code_challenge" value:challenge],
        [NSURLQueryItem queryItemWithName:@"code_challenge_method" value:@"S256"],
        [NSURLQueryItem queryItemWithName:@"prompt" value:@"select_account"],
    ];
    __weak typeof(self) weakSelf = self;
    ASWebAuthenticationSession *session = [[ASWebAuthenticationSession alloc]
        initWithURL:components.URL
        callbackURLScheme:@"nclr"
        completionHandler:^(NSURL *callbackURL, NSError *error) {
            if (error != nil || callbackURL == nil) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    completion(nil, nil, error.localizedDescription ?: @"登录已取消");
                });
                return;
            }
            NSURLComponents *result = [NSURLComponents componentsWithURL:callbackURL
                                                 resolvingAgainstBaseURL:NO];
            NSMutableDictionary *query = [NSMutableDictionary dictionary];
            for (NSURLQueryItem *item in result.queryItems) {
                if (item.name.length > 0 && item.value.length > 0) query[item.name] = item.value;
            }
            NSString *code = query[@"code"];
            NSString *oauthError = query[@"error_description"] ?: query[@"error"];
            if (code.length == 0) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    completion(nil, nil, oauthError ?: @"登录页没有返回授权码");
                });
                return;
            }
            NSString *body = [NSString stringWithFormat:
                @"grant_type=authorization_code&client_id=%@&code=%@&redirect_uri=%@&code_verifier=%@&scope=%@",
                clientId, code, @"nclr%3A%2F%2Fauth", verifier,
                @"offline_access%20XboxLive.signin"];
            [weakSelf postForm:@"https://login.microsoftonline.com/consumers/oauth2/v2.0/token"
                        body:body
                  completion:^(NSDictionary *json, NSString *tokenError) {
                NSString *access = NCLRString(json[@"access_token"]);
                if (tokenError || access.length == 0) {
                    dispatch_async(dispatch_get_main_queue(), ^{
                        completion(nil, nil, tokenError ?: NCLRString(json[@"error_description"]) ?: @"授权码换取令牌失败");
                    });
                    return;
                }
                [weakSelf exchangeMicrosoftToken:access completion:completion];
            }];
        }];
    session.presentationContextProvider = self;
    session.prefersEphemeralWebBrowserSession = NO;
    self.session = session;
    if (![session start]) {
        completion(nil, nil, @"无法启动系统登录浏览器");
    }
}

- (void)startWithClientId:(NSString *)clientId
                presenter:(UIViewController *)presenter
               completion:(NCLRAuthCompletion)completion {
    if (clientId.length == 0) {
        completion(nil, nil, @"请先配置 Azure Client ID");
        return;
    }
    __weak typeof(self) weakSelf = self;
    NSString *body = [NSString stringWithFormat:
        @"client_id=%@&scope=%@",
        clientId,
        @"offline_access%20XboxLive.signin"];
    [self postForm:@"https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode"
             body:body
       completion:^(NSDictionary *json, NSString *error) {
        if (error || json[@"error"]) {
            dispatch_async(dispatch_get_main_queue(), ^{
                completion(nil, nil, error ?: NCLRString(json[@"error_description"]) ?: @"无法开始登录");
            });
            return;
        }
        NSString *deviceCode = NCLRString(json[@"device_code"]);
        NSString *userCode = NCLRString(json[@"user_code"]);
        NSString *verification = NCLRString(json[@"verification_uri"]) ?:
            NCLRString(json[@"verification_url"]);
        NSInteger interval = [json[@"interval"] integerValue];
        if (deviceCode.length == 0 || userCode.length == 0) {
            dispatch_async(dispatch_get_main_queue(), ^{
                completion(nil, nil, @"设备码响应不完整");
            });
            return;
        }
        dispatch_async(dispatch_get_main_queue(), ^{
            UIAlertController *alert = [UIAlertController
                alertControllerWithTitle:@"使用 Microsoft 登录"
                                 message:[NSString stringWithFormat:
                                    @"在浏览器打开登录页，输入代码：\n\n%@", userCode]
                          preferredStyle:UIAlertControllerStyleAlert];
            [alert addAction:[UIAlertAction actionWithTitle:@"复制代码" style:UIAlertActionStyleDefault
                handler:^(UIAlertAction *action) {
                    UIPasteboard.generalPasteboard.string = userCode;
                }]];
            [alert addAction:[UIAlertAction actionWithTitle:@"打开系统登录页" style:UIAlertActionStyleDefault
                handler:^(UIAlertAction *action) {
                    UIPasteboard.generalPasteboard.string = userCode;
                    NSURL *url = verification.length > 0 ? [NSURL URLWithString:verification] : nil;
                    [alert dismissViewControllerAnimated:YES completion:^{
                        if (url != nil) {
                            SFSafariViewController *safari = [[SFSafariViewController alloc] initWithURL:url];
                            weakSelf.authSafari = safari;
                            [presenter presentViewController:safari animated:YES completion:nil];
                        }
                        [weakSelf pollWithClientId:clientId
                                        deviceCode:deviceCode
                                          interval:MAX(interval, 5)
                                         presenter:presenter
                                        completion:completion];
                    }];
                }]];
            [alert addAction:[UIAlertAction actionWithTitle:@"取消" style:UIAlertActionStyleCancel handler:nil]];
            [presenter presentViewController:alert animated:YES completion:nil];
        });
    }];
}

- (void)pollWithClientId:(NSString *)clientId
              deviceCode:(NSString *)deviceCode
                interval:(NSInteger)interval
               presenter:(UIViewController *)presenter
              completion:(NCLRAuthCompletion)completion {
    __weak typeof(self) weakSelf = self;
    NSString *body = [NSString stringWithFormat:
        @"grant_type=urn:ietf:params:oauth:grant-type:device_code&client_id=%@&device_code=%@",
        clientId, deviceCode];
    [self postForm:@"https://login.microsoftonline.com/consumers/oauth2/v2.0/token"
             body:body
       completion:^(NSDictionary *json, NSString *error) {
        if (error) {
            dispatch_async(dispatch_get_main_queue(), ^{ completion(nil, nil, error); });
            return;
        }
        NSString *accessToken = NCLRString(json[@"access_token"]);
        if (accessToken.length > 0) {
            [weakSelf exchangeMicrosoftToken:accessToken completion:completion];
            return;
        }
        NSString *code = NCLRString(json[@"error"]);
        if ([code isEqualToString:@"authorization_pending"]) {
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)interval * NSEC_PER_SEC),
                           dispatch_get_main_queue(), ^{
                [weakSelf pollWithClientId:clientId deviceCode:deviceCode
                                  interval:interval presenter:presenter completion:completion];
            });
            return;
        }
        if ([code isEqualToString:@"slow_down"]) {
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(interval + 5) * NSEC_PER_SEC),
                           dispatch_get_main_queue(), ^{
                [weakSelf pollWithClientId:clientId deviceCode:deviceCode
                                  interval:interval + 5 presenter:presenter completion:completion];
            });
            return;
        }
        NSString *message = NCLRString(json[@"error_description"]) ?: code ?: @"登录未完成";
        dispatch_async(dispatch_get_main_queue(), ^{ completion(nil, nil, message); });
    }];
}

- (void)exchangeMicrosoftToken:(NSString *)microsoftToken completion:(NCLRAuthCompletion)completion {
    NSDictionary *xblBody = @{
        @"Properties" : @{
            @"AuthMethod" : @"RPS",
            @"SiteName" : @"user.auth.xboxlive.com",
            @"RpsTicket" : [NSString stringWithFormat:@"d=%@", microsoftToken],
        },
        @"RelyingParty" : @"http://auth.xboxlive.com",
        @"TokenType" : @"JWT",
    };
    __weak typeof(self) weakSelf = self;
    [self postJSON:@"https://user.auth.xboxlive.com/user/authenticate"
              body:xblBody headers:@{} completion:^(NSDictionary *xbl, NSInteger status, NSString *error) {
        NSString *xblToken = NCLRString(xbl[@"Token"]);
        NSArray *xui = xbl[@"DisplayClaims"][@"xui"];
        NSString *uhs = xui.count > 0 ? NCLRString(xui.firstObject[@"uhs"]) : nil;
        if (error || xblToken.length == 0 || uhs.length == 0) {
            dispatch_async(dispatch_get_main_queue(), ^{
                completion(nil, nil, error ?: @"Xbox Live 认证失败");
            });
            return;
        }
        NSDictionary *xstsBody = @{
            @"Properties" : @{@"SandboxId" : @"RETAIL", @"UserTokens" : @[xblToken]},
            @"RelyingParty" : @"rp://api.minecraftservices.com/",
            @"TokenType" : @"JWT",
        };
        [weakSelf postJSON:@"https://xsts.auth.xboxlive.com/xsts/authorize"
                      body:xstsBody headers:@{} completion:^(NSDictionary *xsts, NSInteger xstsStatus, NSString *xstsError) {
            NSString *xstsToken = NCLRString(xsts[@"Token"]);
            if (xstsError || xstsToken.length == 0) {
                NSString *message = xstsError ?: NCLRString(xsts[@"Message"]) ?:
                    @"XSTS 认证失败（账号可能没有 Minecraft: Java Edition）";
                dispatch_async(dispatch_get_main_queue(), ^{ completion(nil, nil, message); });
                return;
            }
            NSDictionary *mcBody = @{
                @"identityToken" : [NSString stringWithFormat:@"XBL3.0 x=%@;%@", uhs, xstsToken],
            };
            [weakSelf postJSON:@"https://api.minecraftservices.com/authentication/login_with_xbox"
                          body:mcBody headers:@{} completion:^(NSDictionary *mc, NSInteger mcStatus, NSString *mcError) {
                NSString *mcToken = NCLRString(mc[@"access_token"]);
                if (mcError || mcToken.length == 0) {
                    dispatch_async(dispatch_get_main_queue(), ^{
                        completion(nil, nil, mcError ?: @"无法换取 Minecraft token");
                    });
                    return;
                }
                [weakSelf fetchProfileWithToken:mcToken completion:completion];
            }];
        }];
    }];
}

- (void)refreshAppearance:(void (^)(NSDictionary *))completion {
    NSString *uuid = self.uuid;
    NSString *token = self.token;
    if (!uuid.length || !token.length || NCLRTokenExpired(token)) return;
    NSMutableURLRequest *request = [NSMutableURLRequest requestWithURL:
        [NSURL URLWithString:@"https://api.minecraftservices.com/minecraft/profile"]];
    request.timeoutInterval = 10.0;
    [request setValue:[@"Bearer " stringByAppendingString:token] forHTTPHeaderField:@"Authorization"];
    [[NSURLSession.sharedSession dataTaskWithRequest:request
        completionHandler:^(NSData *data, NSURLResponse *response, NSError *error) {
        id profile = !error && [(NSHTTPURLResponse *)response statusCode] == 200 && data
            ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
        dispatch_async(dispatch_get_main_queue(), ^{
            if (![self.uuid isEqualToString:uuid] || ![self.token isEqualToString:token]) return;
            if (![profile isKindOfClass:NSDictionary.class] || ![profile[@"id"] isEqualToString:uuid]) return;
            NCLRSaveAppearance(profile);
            completion(profile);
        });
    }] resume];
}

- (void)fetchProfileWithToken:(NSString *)token completion:(NCLRAuthCompletion)completion {
    __weak typeof(self) weakSelf = self;
    NSMutableURLRequest *request = [NSMutableURLRequest
        requestWithURL:[NSURL URLWithString:@"https://api.minecraftservices.com/minecraft/profile"]];
    [request setValue:[NSString stringWithFormat:@"Bearer %@", token]
   forHTTPHeaderField:@"Authorization"];
    [[NSURLSession.sharedSession dataTaskWithRequest:request
        completionHandler:^(NSData *data, NSURLResponse *response, NSError *error) {
            NSInteger status = [(NSHTTPURLResponse *)response statusCode];
            id profile = data.length ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
            NSString *name = [profile isKindOfClass:NSDictionary.class] ? NCLRString(profile[@"name"]) : nil;
            NSString *uuid = [profile isKindOfClass:NSDictionary.class] ? NCLRString(profile[@"id"]) : nil;
            dispatch_async(dispatch_get_main_queue(), ^{
                NCLRAuth *auth = weakSelf;
                if (error || status != 200 || !NCLRAvatarURLForName(name) || uuid.length != 32) {
                    [auth.authSafari dismissViewControllerAnimated:YES completion:nil];
                    auth.authSafari = nil;
                    completion(nil, nil, error.localizedDescription ?: @"无法读取 Minecraft 正版玩家资料");
                    return;
                }
                NCLRSaveAppearance(profile);
                auth.token = token;
                auth.name = name;
                auth.uuid = uuid;
                auth.avatarURL = NCLRAvatarURLForName(name);
                NCLRKeychainWrite(token);
                NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
                [defaults setObject:name forKey:kNCLRAccountNameKey];
                [defaults setObject:uuid forKey:kNCLRAccountUUIDKey];
                [[NSNotificationCenter defaultCenter] postNotificationName:NCLRAvatarUpdatedNotification object:auth];
                [weakSelf.authSafari dismissViewControllerAnimated:YES completion:nil];
                weakSelf.authSafari = nil;
                completion(token, name, nil);
            });
        }] resume];
}
@end

#pragma mark - UI helpers

@interface NCLRPageController : UIViewController
@property (nonatomic, strong) UIScrollView *scroll;
@property (nonatomic, strong) UIStackView *stack;
@property (nonatomic, strong) NSLayoutConstraint *stackLeading;
@property (nonatomic, strong) NSLayoutConstraint *stackTrailing;
@property (nonatomic, copy) void (^onDataChanged)(void);
- (void)setupPage;
- (UILabel *)label:(NSString *)text size:(CGFloat)size weight:(UIFontWeight)weight color:(UIColor *)color;
- (UIView *)card;
- (UIButton *)button:(NSString *)title action:(SEL)action primary:(BOOL)primary;
- (UILabel *)eyebrow:(NSString *)text;
- (UIView *)headerWithTitle:(NSString *)title
                   subtitle:(NSString *)subtitle
                actionTitle:(NSString *)actionTitle
                     action:(SEL)action;
- (UIView *)rowWithTitle:(NSString *)title detail:(NSString *)detail;
@end

@implementation NCLRPageController
- (void)setupPage {
    self.view.backgroundColor = UIColor.clearColor;
    self.scroll = [[UIScrollView alloc] init];
    self.scroll.translatesAutoresizingMaskIntoConstraints = NO;
    self.scroll.alwaysBounceVertical = YES;
    self.scroll.showsVerticalScrollIndicator = NO;
    [self.view addSubview:self.scroll];
    self.stack = [[UIStackView alloc] init];
    self.stack.axis = UILayoutConstraintAxisVertical;
    self.stack.spacing = 14.0;
    self.stack.translatesAutoresizingMaskIntoConstraints = NO;
    [self.scroll addSubview:self.stack];
    self.stackLeading = [self.stack.leadingAnchor constraintEqualToAnchor:self.scroll.frameLayoutGuide.leadingAnchor constant:24.0];
    self.stackTrailing = [self.stack.trailingAnchor constraintEqualToAnchor:self.scroll.frameLayoutGuide.trailingAnchor constant:-24.0];
    [NSLayoutConstraint activateConstraints:@[
        [self.scroll.leadingAnchor constraintEqualToAnchor:self.view.leadingAnchor],
        [self.scroll.trailingAnchor constraintEqualToAnchor:self.view.trailingAnchor],
        [self.scroll.topAnchor constraintEqualToAnchor:self.view.topAnchor],
        [self.scroll.bottomAnchor constraintEqualToAnchor:self.view.bottomAnchor],
        self.stackLeading,
        self.stackTrailing,
        [self.stack.topAnchor constraintEqualToAnchor:self.scroll.contentLayoutGuide.topAnchor constant:18.0],
        [self.stack.bottomAnchor constraintEqualToAnchor:self.scroll.contentLayoutGuide.bottomAnchor constant:-24.0],
    ]];
}

- (void)viewDidLayoutSubviews {
    [super viewDidLayoutSubviews];
    CGFloat width = self.view.bounds.size.width;
    CGFloat pad = width < 560.0 ? 14.0 : (width < 760.0 ? 18.0 : 24.0);
    self.stackLeading.constant = pad;
    self.stackTrailing.constant = -pad;
    UIEdgeInsets inset = self.scroll.contentInset;
    inset.bottom = MAX(16.0, self.view.safeAreaInsets.bottom + 12.0);
    self.scroll.contentInset = inset;
    self.scroll.verticalScrollIndicatorInsets = inset;
}

- (UILabel *)label:(NSString *)text size:(CGFloat)size weight:(UIFontWeight)weight color:(UIColor *)color {
    UILabel *label = [[UILabel alloc] init];
    label.text = text;
    label.textColor = color;
    label.font = [UIFont systemFontOfSize:size weight:weight];
    label.numberOfLines = 0;
    label.translatesAutoresizingMaskIntoConstraints = NO;
    return label;
}

- (UILabel *)eyebrow:(NSString *)text {
    UILabel *label = [self label:text size:9.0 weight:UIFontWeightBold
                            color:NCLRColor(0.50, 0.50, 0.50, 1.0)];
    label.attributedText = [[NSAttributedString alloc] initWithString:text attributes:@{
        NSKernAttributeName : @1.6
    }];
    return label;
}

- (UIView *)card {
    UIView *card = [[UIView alloc] init];
    card.translatesAutoresizingMaskIntoConstraints = NO;
    card.backgroundColor = NCLRSurface();
    card.layer.cornerRadius = 18.0;
    card.clipsToBounds = YES;
    card.layer.borderWidth = 1.0;
    card.layer.borderColor = NCLRBorder().CGColor;
    return card;
}

- (UIButton *)button:(NSString *)title action:(SEL)action primary:(BOOL)primary {
    UIButton *button = [UIButton buttonWithType:UIButtonTypeSystem];
    [button setTitle:title forState:UIControlStateNormal];
    button.titleLabel.font = [UIFont systemFontOfSize:15.0 weight:UIFontWeightSemibold];
    button.layer.cornerRadius = 12.0;
    button.translatesAutoresizingMaskIntoConstraints = NO;
    [button.heightAnchor constraintGreaterThanOrEqualToConstant:42.0].active = YES;
    if (primary) {
        [button setTitleColor:NCLRDarkMode ? UIColor.blackColor : UIColor.whiteColor forState:UIControlStateNormal];
        button.backgroundColor = NCLRAccent();
        [button setTitleColor:NCLRDarkMode ? UIColor.blackColor : UIColor.whiteColor forState:UIControlStateDisabled];
    } else {
        [button setTitleColor:NCLRColor(0.94, 0.94, 0.94, 1.0) forState:UIControlStateNormal];
        button.backgroundColor = NCLRCanvas();
        button.layer.borderWidth = 1.0;
        button.layer.borderColor = NCLRBorder().CGColor;
    }
    [button addTarget:self action:action forControlEvents:UIControlEventTouchUpInside];
    return button;
}

- (UIView *)headerWithTitle:(NSString *)title
                   subtitle:(NSString *)subtitle
                actionTitle:(NSString *)actionTitle
                     action:(SEL)action {
    UIView *header = [[UIView alloc] init];
    header.translatesAutoresizingMaskIntoConstraints = NO;
    UILabel *titleLabel = [self label:title size:28.0 weight:UIFontWeightBold
                                color:NCLRColor(1.0, 1.0, 1.0, 1.0)];
    UILabel *subtitleLabel = [self label:subtitle size:11.0 weight:UIFontWeightRegular
                                   color:NCLRColor(0.62, 0.62, 0.62, 1.0)];
    [header addSubview:titleLabel];
    [header addSubview:subtitleLabel];
    [NSLayoutConstraint activateConstraints:@[
        [titleLabel.leadingAnchor constraintEqualToAnchor:header.leadingAnchor],
        [titleLabel.topAnchor constraintEqualToAnchor:header.topAnchor],
        [subtitleLabel.leadingAnchor constraintEqualToAnchor:titleLabel.leadingAnchor],
        [subtitleLabel.topAnchor constraintEqualToAnchor:titleLabel.bottomAnchor constant:4.0],
        [subtitleLabel.bottomAnchor constraintEqualToAnchor:header.bottomAnchor],
    ]];
    if (actionTitle.length > 0) {
        UIButton *button = [self button:actionTitle action:action primary:NO];
        [header addSubview:button];
        [NSLayoutConstraint activateConstraints:@[
            [button.trailingAnchor constraintEqualToAnchor:header.trailingAnchor],
            [button.centerYAnchor constraintEqualToAnchor:titleLabel.centerYAnchor],
            [button.widthAnchor constraintGreaterThanOrEqualToConstant:100.0],
        ]];
        [titleLabel.trailingAnchor constraintLessThanOrEqualToAnchor:button.leadingAnchor constant:-12.0].active = YES;
    } else {
        [titleLabel.trailingAnchor constraintLessThanOrEqualToAnchor:header.trailingAnchor].active = YES;
    }
    return header;
}

- (UIView *)rowWithTitle:(NSString *)title detail:(NSString *)detail {
    UIView *row = [[UIView alloc] init];
    row.translatesAutoresizingMaskIntoConstraints = NO;
    UILabel *titleLabel = [self label:title size:14.0 weight:UIFontWeightSemibold
                                color:NCLRColor(0.96, 0.96, 0.96, 1.0)];
    UILabel *detailLabel = [self label:detail size:11.0 weight:UIFontWeightRegular
                                 color:NCLRColor(0.62, 0.62, 0.62, 1.0)];
    [row addSubview:titleLabel];
    [row addSubview:detailLabel];
    [NSLayoutConstraint activateConstraints:@[
        [titleLabel.leadingAnchor constraintEqualToAnchor:row.leadingAnchor],
        [titleLabel.topAnchor constraintEqualToAnchor:row.topAnchor],
        [titleLabel.trailingAnchor constraintLessThanOrEqualToAnchor:row.trailingAnchor],
        [detailLabel.leadingAnchor constraintEqualToAnchor:titleLabel.leadingAnchor],
        [detailLabel.topAnchor constraintEqualToAnchor:titleLabel.bottomAnchor constant:3.0],
        [detailLabel.trailingAnchor constraintLessThanOrEqualToAnchor:row.trailingAnchor],
        [detailLabel.bottomAnchor constraintEqualToAnchor:row.bottomAnchor],
    ]];
    return row;
}
@end

#pragma mark - Pages

// A small, unlit voxel scene. Each face samples the account's original texture.
@interface NCLRPlayerStage : SCNView
- (void)showSkin:(UIImage *)skin cape:(UIImage *)cape slim:(BOOL)slim;
@end

static NSArray<SCNMaterial *> *NCLRPlayerMaterials(UIImage *atlas, NSInteger u, NSInteger v,
                                                     NSInteger w, NSInteger h, NSInteger d,
                                                     CGFloat atlasHeight, UIColor *fallback) {
    CGRect faces[] = {
        CGRectMake(u + d, v + d, w, h), CGRectMake(u + d + w, v + d, d, h),
        CGRectMake(u + 2*d + w, v + d, w, h), CGRectMake(u, v + d, d, h),
        CGRectMake(u + d, v, w, d), CGRectMake(u + d + w, v, w, d),
    };
    NSMutableArray *materials = [NSMutableArray arrayWithCapacity:6];
    for (NSUInteger i = 0; i < 6; i++) {
        SCNMaterial *material = [SCNMaterial material];
        material.lightingModelName = SCNLightingModelConstant;
        material.diffuse.magnificationFilter = SCNFilterModeNearest;
        material.diffuse.minificationFilter = SCNFilterModeNearest;
        material.diffuse.contents = fallback;
        CGImageRef image = atlas.CGImage;
        if (image) {
            CGFloat sx = CGImageGetWidth(image) / 64.0;
            CGFloat sy = CGImageGetHeight(image) / atlasHeight;
            CGRect face = faces[i];
            CGRect pixels = CGRectMake(face.origin.x * sx, face.origin.y * sy, face.size.width * sx, face.size.height * sy);
            CGImageRef crop = CGImageCreateWithImageInRect(image, pixels);
            if (crop) {
                material.diffuse.contents = [UIImage imageWithCGImage:crop];
                CGImageRelease(crop);
            }
        }
        [materials addObject:material];
    }
    return materials;
}

@implementation NCLRPlayerStage {
    SCNNode *_model;
    CGFloat _yaw;
}
- (instancetype)initWithFrame:(CGRect)frame {
    self = [super initWithFrame:frame options:nil];
    if (!self) return nil;
    self.scene = [SCNScene scene];
    self.backgroundColor = UIColor.clearColor;
    self.opaque = NO;
    self.antialiasingMode = SCNAntialiasingModeMultisampling2X;
    self.preferredFramesPerSecond = 30;
    self.accessibilityLabel = @"玩家模型，可拖动旋转";
    self.isAccessibilityElement = YES;
    SCNNode *camera = [SCNNode node];
    camera.camera = [SCNCamera camera];
    camera.camera.usesOrthographicProjection = YES;
    camera.camera.orthographicScale = 21;
    camera.position = SCNVector3Make(0, 6, 50);
    camera.eulerAngles = SCNVector3Make(-0.12, 0, 0);
    [self.scene.rootNode addChildNode:camera];
    self.pointOfView = camera;
    _yaw = -0.55;
    [self addGestureRecognizer:[[UIPanGestureRecognizer alloc] initWithTarget:self action:@selector(rotatePlayer:)]];
    [self showSkin:nil cape:nil slim:NO];
    return self;
}

- (void)didMoveToWindow {
    [super didMoveToWindow];
    self.playing = self.window != nil && !UIAccessibilityIsReduceMotionEnabled();
}

- (void)rotatePlayer:(UIPanGestureRecognizer *)gesture {
    _yaw += [gesture translationInView:self].x * 0.018;
    _model.eulerAngles = SCNVector3Make(0, _yaw, 0);
    [gesture setTranslation:CGPointZero inView:self];
}

- (SCNNode *)boxWithAtlas:(UIImage *)atlas uv:(CGPoint)uv size:(SCNVector3)size
                 position:(SCNVector3)position atlasHeight:(CGFloat)atlasHeight color:(UIColor *)color {
    SCNBox *box = [SCNBox boxWithWidth:size.x height:size.y length:size.z chamferRadius:0];
    box.materials = NCLRPlayerMaterials(atlas, uv.x, uv.y, size.x, size.y, size.z, atlasHeight, color);
    SCNNode *part = [SCNNode nodeWithGeometry:box];
    part.position = position;
    [_model addChildNode:part];
    return part;
}

- (void)showSkin:(UIImage *)skin cape:(UIImage *)cape slim:(BOOL)slim {
    [_model removeFromParentNode];
    _model = [SCNNode node];
    [self.scene.rootNode addChildNode:_model];
    _model.eulerAngles = SCNVector3Make(0, _yaw, 0);
    BOOL legacy = skin && CGImageGetWidth(skin.CGImage) == 2 * CGImageGetHeight(skin.CGImage);
    CGFloat height = legacy ? 32 : 64;
    CGFloat arm = slim ? 3 : 4;
    UIColor *face = [UIColor colorWithRed:0.80 green:0.58 blue:0.44 alpha:1];
    UIColor *shirt = NCLRAccent();
    UIColor *trousers = [UIColor colorWithRed:0.19 green:0.28 blue:0.40 alpha:1];
    [self boxWithAtlas:skin uv:CGPointMake(0,0) size:SCNVector3Make(8,8,8)
              position:SCNVector3Make(0,12,0) atlasHeight:height color:face];
    [self boxWithAtlas:skin uv:CGPointMake(16,16) size:SCNVector3Make(8,12,4)
              position:SCNVector3Make(0,2,0) atlasHeight:height color:shirt];
    [self boxWithAtlas:skin uv:CGPointMake(40,16) size:SCNVector3Make(arm,12,4)
              position:SCNVector3Make(-4-arm/2,2,0) atlasHeight:height color:shirt];
    [self boxWithAtlas:skin uv:legacy ? CGPointMake(40,16) : CGPointMake(32,48) size:SCNVector3Make(arm,12,4)
              position:SCNVector3Make(4+arm/2,2,0) atlasHeight:height color:shirt];
    [self boxWithAtlas:skin uv:CGPointMake(0,16) size:SCNVector3Make(4,12,4)
              position:SCNVector3Make(-2,-10,0) atlasHeight:height color:trousers];
    [self boxWithAtlas:skin uv:legacy ? CGPointMake(0,16) : CGPointMake(16,48) size:SCNVector3Make(4,12,4)
              position:SCNVector3Make(2,-10,0) atlasHeight:height color:trousers];
    if (skin) {
        SCNNode *hat = [self boxWithAtlas:skin uv:CGPointMake(32,0) size:SCNVector3Make(8,8,8)
              position:SCNVector3Make(0,12,0) atlasHeight:height color:UIColor.clearColor];
        hat.scale = SCNVector3Make(1.10,1.10,1.10);
    }
    if (cape) {
        SCNNode *cloth = [self boxWithAtlas:cape uv:CGPointZero size:SCNVector3Make(10,16,1)
              position:SCNVector3Make(0,-0.4,-3.0) atlasHeight:32 color:NCLRAccent()];
        cloth.eulerAngles = SCNVector3Make(0.14,0,0);
    }
    if (!UIAccessibilityIsReduceMotionEnabled()) {
        SCNAction *up = [SCNAction moveByX:0 y:0.45 z:0 duration:1.6];
        up.timingMode = SCNActionTimingModeEaseInEaseOut;
        [_model runAction:[SCNAction repeatActionForever:[SCNAction sequence:@[up, up.reversedAction]]]];
    }
}
@end

@interface NCLRHomeController : NCLRPageController
@property (nonatomic, strong) UILabel *prepareStatusLabel;
@property (nonatomic, strong) UIButton *launchButton;
@property (nonatomic, strong) UIButton *cancelLaunchDownload;
@property (nonatomic, strong) UIProgressView *launchProgress;
@property (nonatomic) BOOL checkingAccount;
@end

@implementation NCLRHomeController
- (void)loadStage:(NCLRPlayerStage *)stage appearance:(NSDictionary *)profile {
    NSDictionary *skin = NCLRActiveTexture(profile, @"skins");
    NSDictionary *cape = NCLRActiveTexture(profile, @"capes");
    BOOL slim = [NCLRString(skin[@"variant"]).lowercaseString isEqualToString:@"slim"];
    __block UIImage *skinImage = nil;
    __block UIImage *capeImage = nil;
    dispatch_group_t textures = dispatch_group_create();
    dispatch_group_enter(textures);
    NCLRFetchTexture(skin, ^(UIImage *image) { skinImage = image; dispatch_group_leave(textures); });
    dispatch_group_enter(textures);
    NCLRFetchTexture(cape, ^(UIImage *image) { capeImage = image; dispatch_group_leave(textures); });
    dispatch_group_notify(textures, dispatch_get_main_queue(), ^{
        if (![NCLRAuth.shared.uuid isEqualToString:profile[@"id"]]) return;
        NSString *uuid = profile[@"id"];
        NSString *skinPath = NCLRTextureCache(uuid, @"skin.png");
        NSString *capePath = NCLRTextureCache(uuid, @"cape.png");
        if (skinPath && skinImage) [UIImagePNGRepresentation(skinImage) writeToFile:skinPath atomically:YES];
        if (capePath && capeImage) [UIImagePNGRepresentation(capeImage) writeToFile:capePath atomically:YES];
        if (capePath && !cape) [NSFileManager.defaultManager removeItemAtPath:capePath error:nil];
        [stage showSkin:skinImage cape:capeImage slim:slim];
    });
}

- (void)viewDidLoad {
    [super viewDidLoad];
    [self setupPage];
    NCLRStore *store = NCLRStore.shared;
    NCLRVersion *selected = store.selected;

    UIView *hero = [self card];
    hero.backgroundColor = NCLRColor(0.055, 0.055, 0.055, 1.0);
    hero.layer.borderColor = NCLRBorder().CGColor;
    UILabel *heroEyebrow = [self eyebrow:@"NCLR · 游戏大厅"];
    heroEyebrow.textColor = NCLRAccent();
    NSString *heroTitleText = selected.name ?: @"Minecraft 26.3";
    UILabel *heroTitle = [self label:heroTitleText
                                size:28.0 weight:UIFontWeightBold
                               color:NCLRColor(1.0, 1.0, 1.0, 1.0)];
    NSString *gameText = selected.gameVersion ?: @"26.3";
    NSString *loaderText = selected.loader ?: @"Vanilla";
    NSString *stateText = selected.installed ? @"已安装" : @"配置文件";
    UILabel *heroDetail = [self label:[NSString stringWithFormat:@"%@ · %@ · %@",
        gameText, loaderText, stateText]
                                  size:11.0 weight:UIFontWeightRegular
                                 color:NCLRColor(0.64, 0.64, 0.64, 1.0)];
    UIButton *launch = [self button:@"启动游戏" action:@selector(launchTapped) primary:YES];
    launch.titleLabel.font = [UIFont systemFontOfSize:17.0 weight:UIFontWeightBold];
    [launch.heightAnchor constraintEqualToConstant:50.0].active = YES;
    self.launchButton = launch;
    self.launchProgress = [[UIProgressView alloc] initWithProgressViewStyle:UIProgressViewStyleDefault];
    self.launchProgress.progressTintColor = NCLRAccent();
    self.launchProgress.trackTintColor = NCLRBorder();
    self.cancelLaunchDownload = [self button:@"取消下载" action:@selector(cancelLaunchPreparation) primary:NO];
    UILabel *hint = [self label:NCLRResourcePreparer.shared.lastLine ?: @""
                           size:12.0 weight:UIFontWeightMedium
                          color:NCLRColor(0.65, 0.65, 0.65, 1.0)];
    self.prepareStatusLabel = hint;
    hint.hidden = hint.text.length == 0;
    UIStackView *heroStack = [[UIStackView alloc] initWithArrangedSubviews:
        @[heroEyebrow, heroTitle, heroDetail, launch, self.launchProgress, hint, self.cancelLaunchDownload]];
    heroStack.axis = UILayoutConstraintAxisVertical;
    heroStack.spacing = 8.0;
    heroStack.translatesAutoresizingMaskIntoConstraints = NO;
    UIView *stagePanel = [UIView new];
    stagePanel.translatesAutoresizingMaskIntoConstraints = NO;
    stagePanel.backgroundColor = NCLRCanvas();
    stagePanel.layer.cornerRadius = 16;
    stagePanel.clipsToBounds = YES;
    UIImageSymbolConfiguration *symbolStyle = [UIImageSymbolConfiguration configurationWithPointSize:108
                                                                                          weight:UIImageSymbolWeightUltraLight];
    UIImageView *orbit = [[UIImageView alloc] initWithImage:[UIImage systemImageNamed:@"circle.dotted"
                                                                   withConfiguration:symbolStyle]];
    orbit.translatesAutoresizingMaskIntoConstraints = NO;
    orbit.tintColor = [NCLRAccent() colorWithAlphaComponent:0.26];
    orbit.contentMode = UIViewContentModeScaleAspectFit;
    [stagePanel addSubview:orbit];
    NCLRPlayerStage *stage = [[NCLRPlayerStage alloc] initWithFrame:CGRectZero];
    stage.translatesAutoresizingMaskIntoConstraints = NO;
    [stagePanel addSubview:stage];
    NCLRAuth *auth = NCLRAuth.shared;
    UILabel *player = [self label:auth.name ?: @"访客" size:13 weight:UIFontWeightSemibold
                             color:NCLRColor(0.94,0.94,0.94,1)];
    player.textAlignment = NSTextAlignmentCenter;
    [stagePanel addSubview:player];
    [hero addSubview:stagePanel];
    [hero addSubview:heroStack];
    [NSLayoutConstraint activateConstraints:@[
        [heroStack.leadingAnchor constraintEqualToAnchor:hero.leadingAnchor constant:20],
        [heroStack.centerYAnchor constraintEqualToAnchor:hero.centerYAnchor],
        [heroStack.topAnchor constraintGreaterThanOrEqualToAnchor:hero.topAnchor constant:18],
        [heroStack.bottomAnchor constraintLessThanOrEqualToAnchor:hero.bottomAnchor constant:-18],
        [stagePanel.leadingAnchor constraintEqualToAnchor:heroStack.trailingAnchor constant:14],
        [stagePanel.trailingAnchor constraintEqualToAnchor:hero.trailingAnchor constant:-12],
        [stagePanel.topAnchor constraintEqualToAnchor:hero.topAnchor constant:12],
        [stagePanel.bottomAnchor constraintEqualToAnchor:hero.bottomAnchor constant:-12],
        [stagePanel.widthAnchor constraintEqualToAnchor:hero.widthAnchor multiplier:0.41],
        [stagePanel.heightAnchor constraintGreaterThanOrEqualToConstant:204],
        [orbit.centerXAnchor constraintEqualToAnchor:stagePanel.centerXAnchor],
        [orbit.centerYAnchor constraintEqualToAnchor:stagePanel.centerYAnchor constant:-6],
        [orbit.widthAnchor constraintEqualToAnchor:stagePanel.widthAnchor multiplier:0.76],
        [orbit.heightAnchor constraintEqualToAnchor:orbit.widthAnchor],
        [stage.leadingAnchor constraintEqualToAnchor:stagePanel.leadingAnchor],
        [stage.trailingAnchor constraintEqualToAnchor:stagePanel.trailingAnchor],
        [stage.topAnchor constraintEqualToAnchor:stagePanel.topAnchor],
        [stage.bottomAnchor constraintEqualToAnchor:player.topAnchor],
        [player.leadingAnchor constraintEqualToAnchor:stagePanel.leadingAnchor constant:8],
        [player.trailingAnchor constraintEqualToAnchor:stagePanel.trailingAnchor constant:-8],
        [player.bottomAnchor constraintEqualToAnchor:stagePanel.bottomAnchor constant:-10],
    ]];
    NSDictionary *cached = NCLRReadAppearance(auth.uuid);
    if (cached) [self loadStage:stage appearance:cached];
    __weak typeof(self) weakSelf = self;
    __weak NCLRPlayerStage *weakStage = stage;
    [auth refreshAppearance:^(NSDictionary *appearance) {
        if (![appearance isEqualToDictionary:cached] && weakStage) {
            [weakSelf loadStage:weakStage appearance:appearance];
        }
    }];
    [self.stack addArrangedSubview:hero];

    UIStackView *stats = [[UIStackView alloc] init];
    stats.axis = UILayoutConstraintAxisHorizontal;
    stats.spacing = 10.0;
    stats.distribution = UIStackViewDistributionFillEqually;
    NSArray *statTitles = @[@"版本", @"模组", @"账号"];
    BOOL premium = NCLRAuth.shared.token.length > 0;
    NSString *accountText = premium
        ? (NCLRAuth.shared.name ?: @"正版用户")
        : @"未登录";
    NSArray *statValues = @[
        [NSString stringWithFormat:@"%lu", (unsigned long)store.versions.count],
        [NSString stringWithFormat:@"%lu", (unsigned long)store.mods.count],
        accountText,
    ];
    for (NSInteger i = 0; i < statTitles.count; i++) {
        UIView *card = [self card];
        UILabel *title = [self eyebrow:statTitles[i]];
        UILabel *value = [self label:statValues[i] size:20.0 weight:UIFontWeightBold
                               color:NCLRColor(1.0, 1.0, 1.0, 1.0)];
        UIStackView *stack = [[UIStackView alloc] initWithArrangedSubviews:@[title, value]];
        stack.axis = UILayoutConstraintAxisVertical;
        stack.spacing = 5.0;
        stack.translatesAutoresizingMaskIntoConstraints = NO;
        [card addSubview:stack];
        [NSLayoutConstraint activateConstraints:@[
            [stack.leadingAnchor constraintEqualToAnchor:card.leadingAnchor constant:14.0],
            [stack.trailingAnchor constraintEqualToAnchor:card.trailingAnchor constant:-14.0],
            [stack.topAnchor constraintEqualToAnchor:card.topAnchor constant:12.0],
            [stack.bottomAnchor constraintEqualToAnchor:card.bottomAnchor constant:-12.0],
        ]];
        [stats addArrangedSubview:card];
    }
    [self.stack addArrangedSubview:stats];

    [self.stack addArrangedSubview:[self eyebrow:@"版本库"]];
    for (NCLRVersion *version in store.versions) {
        UIView *card = [self card];
        UIView *row = [self rowWithTitle:version.name
                                 detail:[NSString stringWithFormat:@"%@ · %@",
                                    version.gameVersion, version.loader]];
        NSString *badgeTitle = [version.identifier isEqualToString:store.selectedVersion]
            ? @"当前" : (version.installed ? @"已安装" : @"配置");
        UIColor *badgeColor = [version.identifier isEqualToString:store.selectedVersion]
            ? NCLRColor(0.95, 0.95, 0.95, 1.0)
            : NCLRColor(0.58, 0.58, 0.58, 1.0);
        UILabel *badge = [self label:badgeTitle
                                size:10.0 weight:UIFontWeightSemibold
                               color:badgeColor];
        [card addSubview:row];
        [card addSubview:badge];
        [NSLayoutConstraint activateConstraints:@[
            [row.leadingAnchor constraintEqualToAnchor:card.leadingAnchor constant:16.0],
            [row.topAnchor constraintEqualToAnchor:card.topAnchor constant:12.0],
            [row.bottomAnchor constraintEqualToAnchor:card.bottomAnchor constant:-12.0],
            [row.trailingAnchor constraintLessThanOrEqualToAnchor:badge.leadingAnchor constant:-12.0],
            [badge.trailingAnchor constraintEqualToAnchor:card.trailingAnchor constant:-16.0],
            [badge.centerYAnchor constraintEqualToAnchor:card.centerYAnchor],
        ]];
        [self.stack addArrangedSubview:card];
    }

    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    [center addObserver:self selector:@selector(prepareProgress:) name:NCLRPrepareProgressNotification object:nil];
    [center addObserver:self selector:@selector(prepareFinished:) name:NCLRPrepareFinishedNotification object:nil];
    [self updateLaunchControlsAnimated:NO];
}

- (void)updateLaunchControlsAnimated:(BOOL)animated {
    NCLRResourcePreparer *preparer = NCLRResourcePreparer.shared;
    BOOL preparing = preparer.preparing;
    NSString *title = self.checkingAccount ? @"正在验证账号…" : @"启动游戏";
    if (preparing) {
        title = preparer.cancelRequested ? @"正在取消…"
            : preparer.cancellable ? @"正在下载游戏资源…" : @"正在组装游戏资源…";
    }
    [self.launchButton setTitle:title forState:UIControlStateNormal];
    self.launchButton.enabled = !preparing && !self.checkingAccount;
    BOOL showProgress = preparing && !preparer.cancelRequested;
    BOOL showCancel = showProgress && preparer.cancellable;
    BOOL changed = (self.launchProgress.hidden == showProgress ||
                    self.cancelLaunchDownload.hidden == showCancel ||
                    self.prepareStatusLabel.hidden == (self.prepareStatusLabel.text.length > 0));
    void (^layout)(void) = ^{
        self.launchProgress.hidden = !showProgress;
        self.cancelLaunchDownload.hidden = !showCancel;
        self.prepareStatusLabel.hidden = self.prepareStatusLabel.text.length == 0;
        [self.view layoutIfNeeded];
    };
    if (changed && animated && !UIAccessibilityIsReduceMotionEnabled()) {
        [UIView animateWithDuration:0.20 delay:0 options:UIViewAnimationOptionBeginFromCurrentState animations:layout completion:nil];
    } else if (changed) {
        layout();
    }
    [self.launchProgress setProgress:preparer.progressValue
                           animated:animated && !UIAccessibilityIsReduceMotionEnabled()];
}

- (void)cancelLaunchPreparation {
    [NCLRResourcePreparer.shared cancel];
    self.prepareStatusLabel.text = @"正在取消，已下载的资源会保留";
    [self updateLaunchControlsAnimated:YES];
}

- (void)showLaunchError:(NSString *)message {
    self.prepareStatusLabel.text = message;
    [self updateLaunchControlsAnimated:YES];
    UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"无法启动游戏" message:message
                                                           preferredStyle:UIAlertControllerStyleAlert];
    [alert addAction:[UIAlertAction actionWithTitle:@"好" style:UIAlertActionStyleDefault handler:nil]];
    [self presentViewController:alert animated:YES completion:nil];
}

- (void)writePerformanceOptions {
    NSString *path = [NCLRDocuments() stringByAppendingPathComponent:@"options.txt"];
    NSMutableArray<NSString *> *lines = [[[NSString stringWithContentsOfFile:path
                                                                  encoding:NSUTF8StringEncoding
                                                                     error:nil]
        componentsSeparatedByString:@"\n"] mutableCopy] ?: [NSMutableArray array];
    NSDictionary<NSString *, NSString *> *wanted = @{
        @"maxFps" : @"120",
        @"batteryMode" : @"false",
        @"enableVsync" : @"true",
    };
    for (NSString *key in wanted) {
        NSString *prefix = [key stringByAppendingString:@":"];
        NSInteger found = -1;
        for (NSInteger i = 0; i < (NSInteger)lines.count; i++) {
            if ([lines[i] hasPrefix:prefix]) {
                found = i;
                break;
            }
        }
        NSString *line = [prefix stringByAppendingString:wanted[key]];
        if (found >= 0) {
            lines[found] = line;
        } else {
            [lines addObject:line];
        }
    }
    [[lines componentsJoinedByString:@"\n"] writeToFile:path atomically:YES
                                                encoding:NSUTF8StringEncoding error:nil];
}

- (void)launchTapped {
    if (NCLRResourcePreparer.shared.preparing || self.checkingAccount) return;
    NCLRAuth *auth = NCLRAuth.shared;
    if (auth.token.length == 0 || NCLRTokenExpired(auth.token)) {
        if (auth.token.length > 0) {
            [auth logout];
            [[NSNotificationCenter defaultCenter] postNotificationName:NCLRAvatarUpdatedNotification object:auth];
        }
        [self showLaunchError:@"请在账号页登录 Microsoft 正版账号后启动游戏。"];
        return;
    }
    if (auth.uuid.length == 0 || !NCLRAvatarURLForName(auth.name)) {
        self.checkingAccount = YES;
        [self updateLaunchControlsAnimated:YES];
        [auth fetchProfileWithToken:auth.token completion:^(NSString *token, NSString *name, NSString *error) {
            self.checkingAccount = NO;
            if (!self.viewIfLoaded.window) return;
            if (error) {
                [self showLaunchError:@"无法确认玩家资料。请检查网络，或退出账号后重新登录。"];
                return;
            }
            [self launchTapped];
        }];
        return;
    }

    NSString *resources = [NCLRDocuments() stringByAppendingPathComponent:@"horizon-client/26.3"];
    NSFileManager *files = NSFileManager.defaultManager;
    NSArray<NSString *> *required = @[@"horizon-resources.json", @"assets/minecraft/sounds.json", @"assets/minecraft/sounds"];
    for (NSString *relative in required) {
        if (![files fileExistsAtPath:[resources stringByAppendingPathComponent:relative]]) {
            [self prepareResources];
            return;
        }
    }

    [self writePerformanceOptions];
    [NCLRKeymap.shared save];
    NSString *game = NCLRDocuments();
    NCLRCreateDirectory(game);
    NSString *sessionPath = [game stringByAppendingPathComponent:@".nclr-session.json"];
    NSData *sessionData = [NSJSONSerialization dataWithJSONObject:@{
        @"name": auth.name, @"uuid": auth.uuid, @"access_token": auth.token, @"user_type": @"msa"
    } options:0 error:nil];
    if (![sessionData writeToFile:sessionPath options:NSDataWritingAtomic | NSDataWritingFileProtectionCompleteUntilFirstUserAuthentication error:nil] ||
        ![files setAttributes:@{NSFilePosixPermissions: @0600} ofItemAtPath:sessionPath error:nil]) {
        [files removeItemAtPath:sessionPath error:nil];
        [self showLaunchError:@"无法建立正版游戏会话，请检查设备可用空间。"];
        return;
    }
    setenv("HORIZON_HOME", game.UTF8String, 1);
    setenv("HORIZON_SESSION_FILE", sessionPath.UTF8String, 1);
    unsetenv("HORIZON_OFFLINE_SKIN");
    unsetenv("HORIZON_OFFLINE_CAPE");
    unsetenv("HORIZON_OFFLINE_SKIN_MODEL");
    NSDictionary *appearance = NCLRReadAppearance(auth.uuid);
    NSString *skinPath = NCLRTextureCache(auth.uuid, @"skin.png");
    NSString *capePath = NCLRTextureCache(auth.uuid, @"cape.png");
    NSDictionary *skin = NCLRActiveTexture(appearance, @"skins");
    if (skin && skinPath && [files fileExistsAtPath:skinPath]) {
        setenv("HORIZON_OFFLINE_SKIN", skinPath.UTF8String, 1);
        BOOL slim = [NCLRString(skin[@"variant"]).lowercaseString isEqualToString:@"slim"];
        setenv("HORIZON_OFFLINE_SKIN_MODEL", slim ? "slim" : "classic", 1);
    }
    if (NCLRActiveTexture(appearance, @"capes") && capePath && [files fileExistsAtPath:capePath]) {
        setenv("HORIZON_OFFLINE_CAPE", capePath.UTF8String, 1);
    }
    NSInteger screenLimit = self.view.window.screen.maximumFramesPerSecond;
    BOOL highRefresh = screenLimit >= 120 && !NSProcessInfo.processInfo.isLowPowerModeEnabled;
    setenv("HORIZON_IOS_FPS", highRefresh ? "120" : "60", 1);
    unsetenv("HORIZON_NO_AUDIO");
    AVAudioSession *audio = AVAudioSession.sharedInstance;
    [audio setCategory:AVAudioSessionCategoryPlayback error:nil];
    [audio setActive:YES error:nil];
    setenv("HORIZON_NO_DISCORD", "1", 1);
    // This call starts winit only now. The patched winit attaches to the
    // already-running UIApplication and drives its loop without a second
    // UIApplicationMain.
    UIWindow *launcherWindow = self.view.window;
    launcherWindow.userInteractionEnabled = NO;
    [launcherWindow resignKeyWindow];
    launcherWindow.hidden = YES;
    NCLRDisplayCadence *cadence = [NCLRDisplayCadence new];
    CADisplayLink *display = [CADisplayLink displayLinkWithTarget:cadence selector:@selector(displayTick:)];
    display.preferredFrameRateRange = highRefresh ? CAFrameRateRangeMake(80, 120, 120)
                                                 : CAFrameRateRangeMake(30, 60, 60);
    [display addToRunLoop:NSRunLoop.mainRunLoop forMode:NSRunLoopCommonModes];
    NCLRKeymap *keymap = NCLRKeymap.shared;
    CMMotionManager *motion = keymap.gyroEnabled
        ? NCLRStartGyro(launcherWindow.windowScene, keymap.gyroSensitivity,
                        keymap.gyroInvertX, keymap.gyroInvertY) : nil;
    [[NCLRHardwareInput shared] start];
    int result = horizon_ios_run(resources.UTF8String, game.UTF8String);
    [[NCLRHardwareInput shared] stop];
    [motion stopDeviceMotionUpdates];
    [display invalidate];
    unsetenv("HORIZON_SESSION_FILE");
    [files removeItemAtPath:sessionPath error:nil];
    [audio setActive:NO error:nil];
    launcherWindow.userInteractionEnabled = YES;
    [launcherWindow makeKeyAndVisible];
    if (result != 0) [self showLaunchError:@"引擎未接受启动请求，请重新登录或重新启动应用。"];
}

- (void)prepareProgress:(NSNotification *)note {
    self.prepareStatusLabel.text = note.userInfo[@"line"];
    [self updateLaunchControlsAnimated:YES];
}

- (void)prepareFinished:(NSNotification *)note {
    self.prepareStatusLabel.text = note.userInfo[@"status"];
    [self updateLaunchControlsAnimated:YES];
}

- (void)dealloc {
    [[NSNotificationCenter defaultCenter] removeObserver:self];
}

- (void)prepareResources {
    if (NCLRResourcePreparer.shared.preparing) return;
    self.prepareStatusLabel.text = @"正在检查游戏资源…";
    [[NCLRResourcePreparer shared] prepareWithProgress:nil completion:^(NSString *error) {
        if (!self.viewIfLoaded.window || [error isEqualToString:@"下载已取消"]) return;
        if (error.length > 0) {
            UIAlertController *fail = [UIAlertController alertControllerWithTitle:@"资源准备失败"
                                                                           message:error
                                                                    preferredStyle:UIAlertControllerStyleAlert];
            [fail addAction:[UIAlertAction actionWithTitle:@"好" style:UIAlertActionStyleDefault handler:nil]];
            [self presentViewController:fail animated:YES completion:nil];
        } else {
            [self launchTapped];
        }
    }];
    [self updateLaunchControlsAnimated:YES];
}
@end

@interface NCLRVersionsController : NCLRPageController
@property (nonatomic, strong) UIView *prepareCard;
@property (nonatomic, strong) UIProgressView *prepareBar;
@property (nonatomic, strong) UILabel *prepareLabel;
@property (nonatomic, strong) UIButton *cancelPrepare;
@end

@implementation NCLRVersionsController
- (void)viewDidLoad {
    [super viewDidLoad];
    [self setupPage];
    [self.stack addArrangedSubview:[self headerWithTitle:@"版本"
                                                subtitle:@"选择、创建和管理游戏配置"
                                             actionTitle:@"添加版本" action:@selector(addVersion)]];
    NCLRStore *store = NCLRStore.shared;
    for (NCLRVersion *version in store.versions) {
        UIView *card = [self card];
        UIView *row = [self rowWithTitle:version.name
                                 detail:[NSString stringWithFormat:@"%@ · %@ · %@",
                                    version.gameVersion, version.loader,
                                    version.installed ? @"已安装" : @"仅配置"]];
        NSString *selectTitle = [version.identifier isEqualToString:store.selectedVersion]
            ? @"当前" : @"选择";
        UIButton *select = [self button:selectTitle
                                     action:@selector(selectTapped:) primary:NO];
        select.tag = [store.versions indexOfObject:version];
        [card addSubview:row];
        [card addSubview:select];
        [NSLayoutConstraint activateConstraints:@[
            [row.leadingAnchor constraintEqualToAnchor:card.leadingAnchor constant:16.0],
            [row.topAnchor constraintEqualToAnchor:card.topAnchor constant:14.0],
            [row.bottomAnchor constraintEqualToAnchor:card.bottomAnchor constant:-14.0],
            [row.trailingAnchor constraintLessThanOrEqualToAnchor:select.leadingAnchor constant:-12.0],
            [select.trailingAnchor constraintEqualToAnchor:card.trailingAnchor constant:-14.0],
            [select.centerYAnchor constraintEqualToAnchor:card.centerYAnchor],
            [select.widthAnchor constraintEqualToConstant:80.0],
        ]];
        [self.stack addArrangedSubview:card];
    }
    UIView *prepare = [self card];
    prepare.hidden = YES;
    UILabel *prepareLabel = [self label:@"准备游戏资源"
                                  size:11.0 weight:UIFontWeightSemibold
                                 color:NCLRColor(0.92, 0.92, 0.92, 1.0)];
    UIButton *cancelPrepare = [self button:@"取消" action:@selector(cancelPrepareTapped) primary:NO];
    cancelPrepare.titleLabel.font = [UIFont systemFontOfSize:11.0 weight:UIFontWeightSemibold];
    [prepare addSubview:cancelPrepare];
    UIProgressView *prepareBar = [[UIProgressView alloc] initWithProgressViewStyle:UIProgressViewStyleDefault];
    prepareBar.translatesAutoresizingMaskIntoConstraints = NO;
    prepareBar.progressTintColor = NCLRColor(0.25, 0.75, 0.48, 1.0);
    prepareBar.trackTintColor = NCLRColor(0.95, 0.95, 0.95, 0.17);
    [prepare addSubview:prepareLabel];
    [prepare addSubview:prepareBar];
    [NSLayoutConstraint activateConstraints:@[
        [prepareLabel.leadingAnchor constraintEqualToAnchor:prepare.leadingAnchor constant:16.0],
        [prepareLabel.trailingAnchor constraintLessThanOrEqualToAnchor:cancelPrepare.leadingAnchor constant:-8.0],
        [cancelPrepare.trailingAnchor constraintEqualToAnchor:prepare.trailingAnchor constant:-14.0],
        [cancelPrepare.centerYAnchor constraintEqualToAnchor:prepare.centerYAnchor],
        [cancelPrepare.widthAnchor constraintEqualToConstant:56.0],
        [cancelPrepare.heightAnchor constraintGreaterThanOrEqualToConstant:44.0],
        [prepare.heightAnchor constraintGreaterThanOrEqualToConstant:56.0],
        [prepareLabel.topAnchor constraintEqualToAnchor:prepare.topAnchor constant:13.0],
        [prepareBar.leadingAnchor constraintEqualToAnchor:prepare.leadingAnchor constant:16.0],
        [prepareBar.trailingAnchor constraintEqualToAnchor:cancelPrepare.leadingAnchor constant:-8.0],
        [prepareBar.topAnchor constraintEqualToAnchor:prepareLabel.bottomAnchor constant:9.0],
        [prepareBar.bottomAnchor constraintEqualToAnchor:prepare.bottomAnchor constant:-14.0],
    ]];
    self.prepareCard = prepare;
    self.prepareLabel = prepareLabel;
    self.cancelPrepare = cancelPrepare;
    self.prepareBar = prepareBar;
    [self.stack addArrangedSubview:prepare];
    [[NSNotificationCenter defaultCenter] addObserver:self
                                             selector:@selector(prepareProgress:)
                                                 name:NCLRPrepareProgressNotification
                                               object:nil];
    [[NSNotificationCenter defaultCenter] addObserver:self
                                             selector:@selector(prepareFinished:)
                                                 name:NCLRPrepareFinishedNotification
                                               object:nil];
    if (NCLRResourcePreparer.shared.preparing) {
        prepare.hidden = NO;
        prepareLabel.text = NCLRResourcePreparer.shared.lastLine ?: @"准备游戏资源";
        prepareBar.progress = NCLRResourcePreparer.shared.progressValue;
        cancelPrepare.enabled = NCLRResourcePreparer.shared.cancellable && !NCLRResourcePreparer.shared.cancelRequested;
        prepare.hidden = NCLRResourcePreparer.shared.cancelRequested;
    }

    UILabel *note = [self label:@"版本配置会保存在本机。实际下载、校验和游戏启动由游戏端完成；启动器不绑定引擎生命周期。"
                            size:10.0 weight:UIFontWeightRegular
                           color:NCLRColor(0.50, 0.50, 0.50, 1.0)];
    [self.stack addArrangedSubview:note];
}

- (void)prepareProgress:(NSNotification *)note {
    self.prepareCard.hidden = NO;
    self.prepareLabel.text = note.userInfo[@"line"] ?: @"准备游戏资源";
    self.prepareBar.progress = NCLRResourcePreparer.shared.progressValue;
    self.cancelPrepare.enabled = NCLRResourcePreparer.shared.cancellable;
}

- (void)prepareFinished:(NSNotification *)note {
    self.prepareCard.hidden = YES;
    self.cancelPrepare.enabled = YES;
}

- (void)dealloc {
    [[NSNotificationCenter defaultCenter] removeObserver:self];
}

- (void)cancelPrepareTapped {
    [[NCLRResourcePreparer shared] cancel];
    self.prepareCard.hidden = YES;
    self.cancelPrepare.enabled = NO;
}

- (void)addVersion {
    UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"添加版本"
                                                                   message:@"创建本地配置。真实文件安装会由游戏端完成。"
                                                            preferredStyle:UIAlertControllerStyleAlert];
    [alert addTextFieldWithConfigurationHandler:^(UITextField *field) {
        field.placeholder = @"名称，例如 Fabric 26.3";
    }];
    [alert addTextFieldWithConfigurationHandler:^(UITextField *field) {
        field.placeholder = @"游戏版本，例如 26.3";
        field.text = @"26.3";
    }];
    [alert addTextFieldWithConfigurationHandler:^(UITextField *field) {
        field.placeholder = @"加载器：Vanilla / Fabric / Forge / NeoForge";
        field.text = @"Vanilla";
    }];
    [alert addAction:[UIAlertAction actionWithTitle:@"取消" style:UIAlertActionStyleCancel handler:nil]];
    [alert addAction:[UIAlertAction actionWithTitle:@"创建" style:UIAlertActionStyleDefault
        handler:^(UIAlertAction *action) {
            NSString *name = alert.textFields[0].text;
            NSString *game = alert.textFields[1].text;
            NSString *loader = alert.textFields[2].text;
            [NCLRStore.shared addVersionNamed:name game:game loader:loader];
            if (self.onDataChanged) self.onDataChanged();
        }]];
    [self presentViewController:alert animated:YES completion:nil];
}

- (void)selectTapped:(UIButton *)sender {
    if (sender.tag < 0 || sender.tag >= (NSInteger)NCLRStore.shared.versions.count) {
        return;
    }
    [NCLRStore.shared selectVersion:NCLRStore.shared.versions[sender.tag]];
    if (self.onDataChanged) self.onDataChanged();
}
@end

@interface NCLRModsController : NCLRPageController <UIDocumentPickerDelegate>
@end

@implementation NCLRModsController
- (void)viewDidLoad {
    [super viewDidLoad];
    [self setupPage];
    [self.stack addArrangedSubview:[self headerWithTitle:@"模组"
                                                subtitle:@"导入、启用和删除本地模组"
                                             actionTitle:@"导入模组" action:@selector(importTapped)]];
    NSString *refusals = NCLRWasmRefusalText();
    if (refusals.length > 0) {
        UIView *warning = [self card];
        UILabel *title = [self label:@"WASM 行为未加载，相关模组内容已禁用"
                                  size:12.0 weight:UIFontWeightSemibold
                                 color:NCLRColor(0.95, 0.42, 0.42, 1.0)];
        UILabel *body = [self label:refusals
                                size:10.0 weight:UIFontWeightRegular
                               color:NCLRColor(0.78, 0.62, 0.62, 1.0)];
        body.numberOfLines = 0;
        UIStackView *stack = [[UIStackView alloc] initWithArrangedSubviews:@[title, body]];
        stack.axis = UILayoutConstraintAxisVertical;
        stack.spacing = 5.0;
        stack.translatesAutoresizingMaskIntoConstraints = NO;
        [warning addSubview:stack];
        [NSLayoutConstraint activateConstraints:@[
            [stack.leadingAnchor constraintEqualToAnchor:warning.leadingAnchor constant:16.0],
            [stack.trailingAnchor constraintEqualToAnchor:warning.trailingAnchor constant:-16.0],
            [stack.topAnchor constraintEqualToAnchor:warning.topAnchor constant:14.0],
            [stack.bottomAnchor constraintEqualToAnchor:warning.bottomAnchor constant:-14.0],
        ]];
        [self.stack addArrangedSubview:warning];
    }

    NCLRStore *store = NCLRStore.shared;
    if (store.mods.count == 0) {
        UIView *card = [self card];
        UILabel *empty = [self label:@"还没有模组。点击右上角导入 .hmod 文件；Java .jar 不会被 Horizon 加载。"
                                size:13.0 weight:UIFontWeightRegular
                               color:NCLRColor(0.65, 0.65, 0.65, 1.0)];
        [card addSubview:empty];
        [NSLayoutConstraint activateConstraints:@[
            [empty.leadingAnchor constraintEqualToAnchor:card.leadingAnchor constant:16.0],
            [empty.trailingAnchor constraintEqualToAnchor:card.trailingAnchor constant:-16.0],
            [empty.topAnchor constraintEqualToAnchor:card.topAnchor constant:18.0],
            [empty.bottomAnchor constraintEqualToAnchor:card.bottomAnchor constant:-18.0],
        ]];
        [self.stack addArrangedSubview:card];
    }
    for (NCLRMod *mod in store.mods) {
        UIView *card = [self card];
        UILabel *name = [self label:mod.name size:14.0 weight:UIFontWeightSemibold
                              color:NCLRColor(0.96, 0.96, 0.96, 1.0)];
        UILabel *detail = [self label:[NSString stringWithFormat:@"horizon-mods/%@ · %llu KB", mod.name, mod.size / 1024]
                                 size:10.0 weight:UIFontWeightRegular
                                color:NCLRColor(0.62, 0.62, 0.62, 1.0)];
        UISwitch *toggle = [[UISwitch alloc] init];
        toggle.translatesAutoresizingMaskIntoConstraints = NO;
        toggle.on = mod.enabled;
        toggle.tag = [store.mods indexOfObject:mod];
        [toggle addTarget:self action:@selector(toggleChanged:) forControlEvents:UIControlEventValueChanged];
        [card addSubview:name];
        [card addSubview:detail];
        [card addSubview:toggle];
        [NSLayoutConstraint activateConstraints:@[
            [name.leadingAnchor constraintEqualToAnchor:card.leadingAnchor constant:16.0],
            [name.topAnchor constraintEqualToAnchor:card.topAnchor constant:12.0],
            [name.trailingAnchor constraintLessThanOrEqualToAnchor:toggle.leadingAnchor constant:-12.0],
            [detail.leadingAnchor constraintEqualToAnchor:name.leadingAnchor],
            [detail.topAnchor constraintEqualToAnchor:name.bottomAnchor constant:3.0],
            [detail.bottomAnchor constraintEqualToAnchor:card.bottomAnchor constant:-12.0],
            [toggle.trailingAnchor constraintEqualToAnchor:card.trailingAnchor constant:-16.0],
            [toggle.centerYAnchor constraintEqualToAnchor:card.centerYAnchor],
        ]];
        [self.stack addArrangedSubview:card];
    }
    UILabel *note = [self label:@"导入目录：Documents/horizon-mods。列表显示已导入文件名；实际加载由游戏端完成。"
                            size:10.0 weight:UIFontWeightRegular
                           color:NCLRColor(0.50, 0.50, 0.50, 1.0)];
    [self.stack addArrangedSubview:note];
}

- (void)importTapped {
    UIDocumentPickerViewController *picker = [[UIDocumentPickerViewController alloc]
        initWithDocumentTypes:@[@"public.data"] inMode:UIDocumentPickerModeImport];
    picker.delegate = self;
    picker.allowsMultipleSelection = NO;
    [self presentViewController:picker animated:YES completion:nil];
}

- (void)documentPicker:(UIDocumentPickerViewController *)controller
    didPickDocumentsAtURLs:(NSArray<NSURL *> *)urls {
    NSString *error = nil;
    if (![NCLRStore.shared importModAtURL:urls.firstObject error:&error]) {
        UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"导入失败"
                                                                       message:error ?: @"无法导入"
                                                                preferredStyle:UIAlertControllerStyleAlert];
        [alert addAction:[UIAlertAction actionWithTitle:@"好" style:UIAlertActionStyleDefault handler:nil]];
        [self presentViewController:alert animated:YES completion:nil];
        return;
    }
    if (self.onDataChanged) self.onDataChanged();
}

- (void)toggleChanged:(UISwitch *)sender {
    if (sender.tag < 0 || sender.tag >= (NSInteger)NCLRStore.shared.mods.count) {
        return;
    }
    [NCLRStore.shared setMod:NCLRStore.shared.mods[sender.tag] enabled:sender.on];
}
@end

@interface NCLRPackController : NCLRPageController <UIDocumentPickerDelegate>
@property (nonatomic, copy) NSString *pageTitle;
@property (nonatomic, copy) NSString *pageSubtitle;
@property (nonatomic, copy) NSString *directoryName;
@property (nonatomic, copy) NSString *emptyText;
@property (nonatomic, copy) NSString *noteText;
@property (nonatomic, copy) NSArray<NSString *> *extensions;
@end

@implementation NCLRPackController
- (NSString *)directoryPath {
    NSString *path = [NCLRDocuments() stringByAppendingPathComponent:self.directoryName ?: @""];
    NCLRCreateDirectory(path);
    return path;
}

- (BOOL)acceptsURL:(NSURL *)url {
    NSNumber *isDirectory = nil;
    [url getResourceValue:&isDirectory forKey:NSURLIsDirectoryKey error:nil];
    if (isDirectory.boolValue) {
        return YES;
    }
    NSString *ext = url.pathExtension.lowercaseString;
    return [self.extensions containsObject:ext];
}

- (void)viewDidLoad {
    [super viewDidLoad];
    [self setupPage];
    [self.stack addArrangedSubview:[self headerWithTitle:self.pageTitle ?: @"文件"
                                                subtitle:self.pageSubtitle ?: @"导入本地文件"
                                             actionTitle:@"导入" action:@selector(importTapped)]];
    NSArray<NSString *> *files = [[NSFileManager defaultManager]
        contentsOfDirectoryAtPath:self.directoryPath error:nil];
    NSMutableArray<NSString *> *shown = [NSMutableArray array];
    for (NSString *file in files) {
        if ([file hasPrefix:@"."]) {
            continue;
        }
        NSString *path = [self.directoryPath stringByAppendingPathComponent:file];
        BOOL directory = NO;
        [[NSFileManager defaultManager] fileExistsAtPath:path isDirectory:&directory];
        NSString *ext = file.pathExtension.lowercaseString;
        if (!directory && ![self.extensions containsObject:ext]) {
            continue;
        }
        [shown addObject:file];
    }
    [shown sortUsingSelector:@selector(localizedCaseInsensitiveCompare:)];
    if (shown.count == 0) {
        UIView *card = [self card];
        UILabel *empty = [self label:self.emptyText ?: @"还没有文件。点击右上角导入。"
                                size:13.0 weight:UIFontWeightRegular
                               color:NCLRColor(0.65, 0.65, 0.65, 1.0)];
        [card addSubview:empty];
        [NSLayoutConstraint activateConstraints:@[
            [empty.leadingAnchor constraintEqualToAnchor:card.leadingAnchor constant:16.0],
            [empty.trailingAnchor constraintEqualToAnchor:card.trailingAnchor constant:-16.0],
            [empty.topAnchor constraintEqualToAnchor:card.topAnchor constant:18.0],
            [empty.bottomAnchor constraintEqualToAnchor:card.bottomAnchor constant:-18.0],
        ]];
        [self.stack addArrangedSubview:card];
    }
    for (NSString *file in shown) {
        NSString *path = [self.directoryPath stringByAppendingPathComponent:file];
        NSDictionary *attributes = [[NSFileManager defaultManager] attributesOfItemAtPath:path error:nil];
        unsigned long long size = [attributes[NSFileSize] unsignedLongLongValue];
        BOOL directory = [attributes[NSFileType] isEqualToString:NSFileTypeDirectory];
        UIView *card = [self card];
        UILabel *name = [self label:file size:14.0 weight:UIFontWeightSemibold
                              color:NCLRColor(0.96, 0.96, 0.96, 1.0)];
        NSString *detailText = directory
            ? [NSString stringWithFormat:@"%@/%@ · 文件夹", self.directoryName, file]
            : [NSString stringWithFormat:@"%@/%@ · %llu KB", self.directoryName, file, size / 1024];
        UILabel *detail = [self label:detailText size:10.0 weight:UIFontWeightRegular
                                color:NCLRColor(0.62, 0.62, 0.62, 1.0)];
        [card addSubview:name];
        [card addSubview:detail];
        [NSLayoutConstraint activateConstraints:@[
            [name.leadingAnchor constraintEqualToAnchor:card.leadingAnchor constant:16.0],
            [name.trailingAnchor constraintLessThanOrEqualToAnchor:card.trailingAnchor constant:-16.0],
            [name.topAnchor constraintEqualToAnchor:card.topAnchor constant:12.0],
            [detail.leadingAnchor constraintEqualToAnchor:name.leadingAnchor],
            [detail.trailingAnchor constraintLessThanOrEqualToAnchor:card.trailingAnchor constant:-16.0],
            [detail.topAnchor constraintEqualToAnchor:name.bottomAnchor constant:3.0],
            [detail.bottomAnchor constraintEqualToAnchor:card.bottomAnchor constant:-12.0],
        ]];
        [self.stack addArrangedSubview:card];
    }
    UILabel *note = [self label:(self.noteText ?: [NSString stringWithFormat:@"目录：Documents/%@", self.directoryName])
                            size:10.0 weight:UIFontWeightRegular
                           color:NCLRColor(0.50, 0.50, 0.50, 1.0)];
    [self.stack addArrangedSubview:note];
}

- (void)importTapped {
    UIDocumentPickerViewController *picker = [[UIDocumentPickerViewController alloc]
        initWithDocumentTypes:@[@"public.data"] inMode:UIDocumentPickerModeImport];
    picker.delegate = self;
    picker.allowsMultipleSelection = NO;
    [self presentViewController:picker animated:YES completion:nil];
}

- (void)documentPicker:(UIDocumentPickerViewController *)controller
    didPickDocumentsAtURLs:(NSArray<NSURL *> *)urls {
    NSURL *url = urls.firstObject;
    if (url == nil || ![self acceptsURL:url]) {
        UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"导入失败"
                                                                       message:[NSString stringWithFormat:@"只接受这些格式：%@",
                                                                                 [self.extensions componentsJoinedByString:@", "]]
                                                                preferredStyle:UIAlertControllerStyleAlert];
        [alert addAction:[UIAlertAction actionWithTitle:@"好" style:UIAlertActionStyleDefault handler:nil]];
        [self presentViewController:alert animated:YES completion:nil];
        return;
    }
    BOOL scoped = [url startAccessingSecurityScopedResource];
    NSString *destination = [self.directoryPath stringByAppendingPathComponent:url.lastPathComponent];
    [[NSFileManager defaultManager] removeItemAtPath:destination error:nil];
    NSError *error = nil;
    BOOL ok = [[NSFileManager defaultManager] copyItemAtURL:url
                                                     toURL:[NSURL fileURLWithPath:destination]
                                                     error:&error];
    if (scoped) {
        [url stopAccessingSecurityScopedResource];
    }
    if (!ok) {
        UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"导入失败"
                                                                       message:error.localizedDescription ?: @"无法复制文件"
                                                                preferredStyle:UIAlertControllerStyleAlert];
        [alert addAction:[UIAlertAction actionWithTitle:@"好" style:UIAlertActionStyleDefault handler:nil]];
        [self presentViewController:alert animated:YES completion:nil];
        return;
    }
    if (self.onDataChanged) self.onDataChanged();
}
@end

@interface NCLRAccountController : NCLRPageController
@end

@implementation NCLRAccountController
- (void)viewDidLoad {
    [super viewDidLoad];
    [self setupPage];
    [self.stack addArrangedSubview:[self headerWithTitle:@"账号" subtitle:@"Microsoft / Minecraft 正版账号"
                                             actionTitle:nil action:nil]];
    NCLRAuth *auth = NCLRAuth.shared;
    BOOL premium = auth.token.length > 0;
    UIView *account = [self card];
    NSString *avatarText = premium ? [(auth.name ?: @"正") substringToIndex:1] : @"N";
    UILabel *avatar = [self label:avatarText
                             size:24.0 weight:UIFontWeightBold
                            color:NCLRColor(0.95, 0.95, 0.95, 1.0)];
    avatar.textAlignment = NSTextAlignmentCenter;
    avatar.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.06];
    avatar.layer.cornerRadius = 23.0;
    avatar.layer.masksToBounds = YES;
    avatar.translatesAutoresizingMaskIntoConstraints = NO;
    UIImageView *avatarImage = [[UIImageView alloc] init];
    avatarImage.translatesAutoresizingMaskIntoConstraints = NO;
    avatarImage.contentMode = UIViewContentModeScaleAspectFill;
    avatarImage.layer.cornerRadius = 23.0;
    avatarImage.layer.masksToBounds = YES;
    avatarImage.hidden = YES;
    [account addSubview:avatarImage];
    NCLRLoadRemoteAvatar(auth.avatarURL, avatarImage, avatar);
    NSString *nameText = premium ? (auth.name ?: @"正版用户") : @"未登录";
    UILabel *name = [self label:nameText
                            size:20.0 weight:UIFontWeightSemibold
                           color:NCLRColor(1.0, 1.0, 1.0, 1.0)];
    NSString *detailText = auth.token.length > 0
        ? @"Minecraft Services token 已保存在 Keychain"
        : @"未登录。登录后才能准备资源并启动游戏。";
    UILabel *detail = [self label:detailText
                             size:11.0 weight:UIFontWeightRegular
                            color:NCLRColor(0.58, 0.58, 0.58, 1.0)];
    NSString *loginText = auth.token.length > 0 ? @"退出登录" : @"登录正式账号";
    UIButton *login = [self button:loginText
                            action:@selector(loginTapped) primary:auth.token.length == 0];
    [account addSubview:avatar];
    [account addSubview:avatarImage];
    [account addSubview:name];
    [account addSubview:detail];
    [account addSubview:login];
    [NSLayoutConstraint activateConstraints:@[
        [avatar.leadingAnchor constraintEqualToAnchor:account.leadingAnchor constant:16.0],
        [avatar.topAnchor constraintEqualToAnchor:account.topAnchor constant:16.0],
        [avatar.widthAnchor constraintEqualToConstant:46.0],
        [avatar.heightAnchor constraintEqualToConstant:46.0],
        [avatarImage.leadingAnchor constraintEqualToAnchor:avatar.leadingAnchor],
        [avatarImage.topAnchor constraintEqualToAnchor:avatar.topAnchor],
        [avatarImage.widthAnchor constraintEqualToAnchor:avatar.widthAnchor],
        [avatarImage.heightAnchor constraintEqualToAnchor:avatar.heightAnchor],
        [name.leadingAnchor constraintEqualToAnchor:avatar.trailingAnchor constant:12.0],
        [name.topAnchor constraintEqualToAnchor:avatar.topAnchor constant:1.0],
        [name.trailingAnchor constraintEqualToAnchor:account.trailingAnchor constant:-16.0],
        [detail.leadingAnchor constraintEqualToAnchor:name.leadingAnchor],
        [detail.topAnchor constraintEqualToAnchor:name.bottomAnchor constant:4.0],
        [detail.trailingAnchor constraintEqualToAnchor:account.trailingAnchor constant:-16.0],
        [login.leadingAnchor constraintEqualToAnchor:account.leadingAnchor constant:16.0],
        [login.trailingAnchor constraintEqualToAnchor:account.trailingAnchor constant:-16.0],
        [login.topAnchor constraintEqualToAnchor:avatar.bottomAnchor constant:14.0],
        [login.bottomAnchor constraintEqualToAnchor:account.bottomAnchor constant:-16.0],
    ]];
    [self.stack addArrangedSubview:account];

}

- (void)configClientId {
    UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"Azure Client ID"
                                                                   message:@"填入你自己的 Azure Application (client) ID。"
                                                            preferredStyle:UIAlertControllerStyleAlert];
    [alert addTextFieldWithConfigurationHandler:^(UITextField *field) {
        field.placeholder = @"00000000-0000-0000-0000-000000000000";
        field.text = NCLRStore.shared.clientId;
        field.autocapitalizationType = UITextAutocapitalizationTypeNone;
    }];
    [alert addAction:[UIAlertAction actionWithTitle:@"取消" style:UIAlertActionStyleCancel handler:nil]];
    [alert addAction:[UIAlertAction actionWithTitle:@"保存" style:UIAlertActionStyleDefault
        handler:^(UIAlertAction *action) {
            NCLRStore *store = NCLRStore.shared;
            store.clientId = [alert.textFields.firstObject.text
                stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]] ?: @"";
            [store save];
            if (self.onDataChanged) self.onDataChanged();
        }]];
    [self presentViewController:alert animated:YES completion:nil];
}

- (void)loginTapped {
    if (NCLRAuth.shared.token.length > 0) {
        [NCLRAuth.shared logout];
        if (self.onDataChanged) self.onDataChanged();
        return;
    }
    if (NCLRStore.shared.clientId.length == 0) {
        NCLRStore.shared.clientId = kNCLRDefaultClientID;
    }
    __weak typeof(self) weakSelf = self;
    [NCLRAuth.shared startWithBrowserClientId:NCLRStore.shared.clientId
                             presenter:self
                            completion:^(NSString *token, NSString *name, NSString *error) {
        if (error) {
            UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"登录失败"
                                                                           message:error
                                                                    preferredStyle:UIAlertControllerStyleAlert];
            [alert addAction:[UIAlertAction actionWithTitle:@"好" style:UIAlertActionStyleDefault handler:nil]];
            [weakSelf presentViewController:alert animated:YES completion:nil];
            return;
        }
        if (weakSelf.onDataChanged) weakSelf.onDataChanged();
    }];
}
@end

@interface NCLRKeyPickerController : UIViewController
@property (nonatomic, copy) void (^onPick)(NSString *key);
+ (void)presentFrom:(UIViewController *)presenter
              title:(NSString *)title
             onPick:(void (^)(NSString *key))onPick;
@end

@implementation NCLRKeyPickerController

+ (void)presentFrom:(UIViewController *)presenter
              title:(NSString *)title
             onPick:(void (^)(NSString *key))onPick {
    NCLRKeyPickerController *picker = [[NCLRKeyPickerController alloc] init];
    picker.onPick = onPick;
    picker.title = title;
    UINavigationController *navigation =
        [[UINavigationController alloc] initWithRootViewController:picker];
    navigation.modalPresentationStyle = UIModalPresentationPageSheet;
    [presenter presentViewController:navigation animated:YES completion:nil];
}

- (void)viewDidLoad {
    [super viewDidLoad];
    self.view.backgroundColor = NCLRColor(0.04, 0.04, 0.04, 1.0);
    self.navigationItem.title = self.title.length > 0 ? self.title : @"选择按键";
    self.navigationItem.rightBarButtonItem =
        [[UIBarButtonItem alloc] initWithTitle:@"关闭"
                                         style:UIBarButtonItemStylePlain
                                        target:self
                                        action:@selector(close)];
    NSArray<NSArray<NSString *> *> *rows = @[
        @[@"Esc", @"1", @"2", @"3", @"4", @"5", @"6", @"7", @"8", @"9"],
        @[@"Tab", @"Q", @"W", @"E", @"R", @"T", @"Y", @"U", @"I", @"O", @"P"],
        @[@"Shift", @"A", @"S", @"D", @"F", @"G", @"H", @"J", @"K", @"L"],
        @[@"Ctrl", @"Z", @"X", @"C", @"V", @"B", @"N", @"M"],
        @[@"键盘", @"Space", @"F1", @"F3", @"F5", @"F"],
        @[@"MouseLeft", @"MouseRight", @"MouseMiddle"],
    ];
    UIStackView *stack = [[UIStackView alloc] init];
    stack.axis = UILayoutConstraintAxisVertical;
    stack.spacing = 7.0;
    stack.translatesAutoresizingMaskIntoConstraints = NO;
    [self.view addSubview:stack];
    for (NSArray<NSString *> *row in rows) {
        UIStackView *line = [[UIStackView alloc] init];
        line.axis = UILayoutConstraintAxisHorizontal;
        line.spacing = 6.0;
        line.distribution = UIStackViewDistributionFillEqually;
        line.translatesAutoresizingMaskIntoConstraints = NO;
        for (NSString *name in row) {
            UIButton *key = [UIButton buttonWithType:UIButtonTypeSystem];
            [key setTitle:name forState:UIControlStateNormal];
            [key setTitleColor:NCLRColor(0.95, 0.95, 0.95, 1.0) forState:UIControlStateNormal];
            key.titleLabel.font = [UIFont monospacedSystemFontOfSize:13.0 weight:UIFontWeightSemibold];
            key.backgroundColor = NCLRColor(0.12, 0.12, 0.12, 1.0);
            key.layer.cornerRadius = 8.0;
            key.layer.borderWidth = 1.0;
            key.layer.borderColor = [UIColor colorWithWhite:1.0 alpha:0.16].CGColor;
            [key.heightAnchor constraintEqualToConstant:44.0].active = YES;
            [key addTarget:self action:@selector(pick:) forControlEvents:UIControlEventTouchUpInside];
            [line addArrangedSubview:key];
        }
        [stack addArrangedSubview:line];
    }
    [NSLayoutConstraint activateConstraints:@[
        [stack.leadingAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.leadingAnchor constant:12.0],
        [stack.trailingAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.trailingAnchor constant:-12.0],
        [stack.topAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.topAnchor constant:14.0],
    ]];
}

- (void)pick:(UIButton *)sender {
    NSString *key = [sender titleForState:UIControlStateNormal];
    if (self.onPick != nil) self.onPick(key);
    [self dismissViewControllerAnimated:YES completion:nil];
}

- (void)close {
    [self dismissViewControllerAnimated:YES completion:nil];
}
@end

@interface NCLRKeymapController : NCLRPageController <UIDocumentPickerDelegate>
@property (nonatomic, strong) UISlider *sizeSlider;
@property (nonatomic, strong) UILabel *sizeValue;
@property (nonatomic, strong) UIButton *menuToggle;
@property (nonatomic, strong) UIStackView *sizeRow;
@property (nonatomic, strong) UISegmentedControl *sizePresetControl;
@property (nonatomic, strong) UISlider *opacitySlider;
@property (nonatomic, strong) UILabel *opacityValue;
@property (nonatomic, strong) UISegmentedControl *modeControl;
@property (nonatomic, strong) UISwitch *dynamicSwitch;
@property (nonatomic, strong) UISegmentedControl *presetControl;
@property (nonatomic, strong) UISegmentedControl *styleControl;
@property (nonatomic, strong) UISwitch *leftHandedSwitch;
@property (nonatomic, strong) UISwitch *joystickSprintSwitch;
@property (nonatomic, strong) UISlider *sensitivitySlider;
@property (nonatomic, strong) UILabel *sensitivityValue;
@property (nonatomic, strong) UISlider *deadzoneSlider;
@property (nonatomic, strong) UILabel *deadzoneValue;
@property (nonatomic, strong) UISwitch *sprintButtonSwitch;
@property (nonatomic, strong) UISwitch *iconLabelsSwitch;
@property (nonatomic, strong) NSMutableDictionary<NSString *, UISwitch *> *visibilitySwitches;
@property (nonatomic, strong) NSMutableArray<UIButton *> *keyButtons;
@property (nonatomic, strong) UIView *preview;
@property (nonatomic, strong) UIView *grid;
@property (nonatomic, strong) UIView *selection;
@property (nonatomic, strong) UIView *resizeHandle;
@property (nonatomic, strong) NSMutableDictionary<NSString *, UIButton *> *previewButtons;
@property (nonatomic, copy) NSString *selectedKey;
@property (nonatomic, assign) CGPoint dragStartCenter;
@property (nonatomic, strong) NSLayoutConstraint *previewHeight;
@property (nonatomic, strong) UIView *previewCard;
@property (nonatomic, strong) UIButton *canvasClose;
@property (nonatomic, strong) UIButton *undoTool;
@property (nonatomic, strong) UIButton *redoTool;
@property (nonatomic, strong) UIButton *gridTool;
@property (nonatomic, strong) UIView *touchSettings;
@property (nonatomic, strong) UIStackView *editorWorkspace;
@property (nonatomic, strong) UIView *editorInspector;
@property (nonatomic, strong) NSLayoutConstraint *inspectorWidth;
@property (nonatomic, strong) UISlider *gyroSlider;
@property (nonatomic, strong) UISwitch *gyroInvertXSwitch;
@property (nonatomic, strong) UISwitch *gyroInvertYSwitch;
@property (nonatomic, strong) UILabel *gyroValue;
@property (nonatomic, strong) UILabel *canvasStatus;
@property (nonatomic, assign) BOOL canvasFullscreen;
@property (nonatomic, strong) NSMutableArray<NSDictionary *> *undoStack;
@property (nonatomic, strong) NSMutableArray<NSDictionary *> *redoStack;
@property (nonatomic, strong) NSDictionary *gestureStartPositions;
@property (nonatomic, strong) UILabel *keyEditTitle;
@property (nonatomic, strong) UISlider *keySizeSlider;
@property (nonatomic, strong) UISlider *keyWidthSlider;
@property (nonatomic, strong) UISegmentedControl *triggerControl;
@property (nonatomic, strong) UIButton *deleteKeyButton;
@property (nonatomic, strong) UISegmentedControl *layoutScreenControl;
@property (nonatomic, strong) UISlider *keyXSlider;
@property (nonatomic, strong) UISlider *keyYSlider;
@property (nonatomic, strong) UILabel *keyCoordinates;
@property (nonatomic, strong) UILabel *keySizeValue;
@property (nonatomic, strong) UILabel *keyWidthValue;
@property (nonatomic, strong) UISwitch *keyVisibleSwitch;
@end

@interface NCLRSettingsController : NCLRPageController
@end

@implementation NCLRSettingsController
- (void)viewDidLoad {
    [super viewDidLoad];
    [self setupPage];
    [self.stack addArrangedSubview:[self headerWithTitle:@"设置" subtitle:@"启动器、存储和关于"
                                             actionTitle:nil action:nil]];

    UIView *theme = [self card];
    UILabel *themeTitle = [self eyebrow:@"外观"];
    UILabel *themeText = [self label:@"切换启动器的黑夜 / 白天配色，设置会保存。"
                                size:12.0 weight:UIFontWeightRegular
                               color:NCLRColor(0.70, 0.70, 0.70, 1.0)];
    UISegmentedControl *themeControl = [[UISegmentedControl alloc] initWithItems:@[@"黑夜", @"白天"]];
    themeControl.selectedSegmentIndex = NCLRDarkMode ? 0 : 1;
    [themeControl addTarget:self action:@selector(themeChanged:) forControlEvents:UIControlEventValueChanged];
    UIStackView *themeStack = [[UIStackView alloc] initWithArrangedSubviews:@[themeTitle, themeText, themeControl]];
    themeStack.axis = UILayoutConstraintAxisVertical;
    themeStack.spacing = 8.0;
    themeStack.translatesAutoresizingMaskIntoConstraints = NO;
    [theme addSubview:themeStack];
    [NSLayoutConstraint activateConstraints:@[
        [themeStack.leadingAnchor constraintEqualToAnchor:theme.leadingAnchor constant:16.0],
        [themeStack.trailingAnchor constraintEqualToAnchor:theme.trailingAnchor constant:-16.0],
        [themeStack.topAnchor constraintEqualToAnchor:theme.topAnchor constant:14.0],
        [themeStack.bottomAnchor constraintEqualToAnchor:theme.bottomAnchor constant:-14.0],
    ]];
    [self.stack addArrangedSubview:theme];

    UIView *mode = [self card];
    UILabel *modeTitle = [self eyebrow:@"启动器架构"];
    UILabel *modeText = [self label:@"NCLR 只运行启动器本身：账号、版本、模组、设置都在这里管理。游戏端是独立应用，通过 nclr-game:// 接收版本；不需要退出启动器，也不再使用单生命周期切换。"
                                  size:12.0 weight:UIFontWeightRegular
                                 color:NCLRColor(0.70, 0.70, 0.70, 1.0)];
    UIStackView *modeStack = [[UIStackView alloc] initWithArrangedSubviews:@[modeTitle, modeText]];
    modeStack.axis = UILayoutConstraintAxisVertical;
    modeStack.spacing = 6.0;
    modeStack.translatesAutoresizingMaskIntoConstraints = NO;
    [mode addSubview:modeStack];
    [NSLayoutConstraint activateConstraints:@[
        [modeStack.leadingAnchor constraintEqualToAnchor:mode.leadingAnchor constant:16.0],
        [modeStack.trailingAnchor constraintEqualToAnchor:mode.trailingAnchor constant:-16.0],
        [modeStack.topAnchor constraintEqualToAnchor:mode.topAnchor constant:14.0],
        [modeStack.bottomAnchor constraintEqualToAnchor:mode.bottomAnchor constant:-14.0],
    ]];
    [self.stack addArrangedSubview:mode];

    UIView *storage = [self card];
    UILabel *storageTitle = [self eyebrow:@"存储"];
    UILabel *storagePath = [self label:NCLRDocuments() size:10.0 weight:UIFontWeightRegular
                                  color:NCLRColor(0.60, 0.60, 0.60, 1.0)];
    storagePath.font = [UIFont monospacedSystemFontOfSize:10.0 weight:UIFontWeightRegular];
    UILabel *storageDetail = [self label:@"版本、mods 和 launcher.json 都在 Documents；可在文件 App 中查看。"
                                    size:11.0 weight:UIFontWeightRegular
                                   color:NCLRColor(0.58, 0.58, 0.58, 1.0)];
    UIStackView *storageStack = [[UIStackView alloc] initWithArrangedSubviews:
        @[storageTitle, storagePath, storageDetail]];
    storageStack.axis = UILayoutConstraintAxisVertical;
    storageStack.spacing = 5.0;
    storageStack.translatesAutoresizingMaskIntoConstraints = NO;
    [storage addSubview:storageStack];
    [NSLayoutConstraint activateConstraints:@[
        [storageStack.leadingAnchor constraintEqualToAnchor:storage.leadingAnchor constant:16.0],
        [storageStack.trailingAnchor constraintEqualToAnchor:storage.trailingAnchor constant:-16.0],
        [storageStack.topAnchor constraintEqualToAnchor:storage.topAnchor constant:14.0],
        [storageStack.bottomAnchor constraintEqualToAnchor:storage.bottomAnchor constant:-14.0],
    ]];
    [self.stack addArrangedSubview:storage];

    UIView *about = [self card];
    UILabel *aboutTitle = [self eyebrow:@"关于"];
    UILabel *aboutText = [self label:@"NCLR Launcher · 正式版\n作者 ZRXMAXLNX、可心\nMinecraft 是 Mojang / Microsoft 的产品；本项目不是官方产品。"
                                  size:11.0 weight:UIFontWeightRegular
                                 color:NCLRColor(0.65, 0.65, 0.65, 1.0)];
    UIStackView *aboutStack = [[UIStackView alloc] initWithArrangedSubviews:@[aboutTitle, aboutText]];
    aboutStack.axis = UILayoutConstraintAxisVertical;
    aboutStack.spacing = 6.0;
    aboutStack.translatesAutoresizingMaskIntoConstraints = NO;
    [about addSubview:aboutStack];
    [NSLayoutConstraint activateConstraints:@[
        [aboutStack.leadingAnchor constraintEqualToAnchor:about.leadingAnchor constant:16.0],
        [aboutStack.trailingAnchor constraintEqualToAnchor:about.trailingAnchor constant:-16.0],
        [aboutStack.topAnchor constraintEqualToAnchor:about.topAnchor constant:14.0],
        [aboutStack.bottomAnchor constraintEqualToAnchor:about.bottomAnchor constant:-14.0],
    ]];
    [self.stack addArrangedSubview:about];

    UIButton *keymap = [self button:@"触控界面" action:@selector(openKeymap) primary:NO];
    [self.stack addArrangedSubview:keymap];

    UIButton *clear = [self button:@"清除本地启动器数据" action:@selector(clearData) primary:NO];
    clear.layer.borderColor = NCLRColor(0.95, 0.35, 0.35, 0.5).CGColor;
    [self.stack addArrangedSubview:clear];
}

- (void)themeChanged:(UISegmentedControl *)control {
    NCLRThemeSet(control.selectedSegmentIndex == 0);
    UIWindow *window = self.view.window;
    window.overrideUserInterfaceStyle = NCLRDarkMode ? UIUserInterfaceStyleDark : UIUserInterfaceStyleLight;
    Class rootClass = NSClassFromString(@"NCLRRootController");
    if (rootClass != Nil) {
        NSInteger page = [[window.rootViewController valueForKey:@"page"] integerValue];
        id root = [[rootClass alloc] init];
        [root setValue:@(page) forKey:@"initialPage"];
        window.rootViewController = root;
    }
}

- (void)openKeymap {
    NCLRKeymapController *controller = [[NCLRKeymapController alloc] init];
    controller.modalPresentationStyle = UIModalPresentationFullScreen;
    [self presentViewController:controller animated:YES completion:nil];
}

- (void)clearData {
    UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"清除本地数据"
                                                                   message:@"会删除启动器配置、版本配置和模组文件，且不可恢复。"
                                                            preferredStyle:UIAlertControllerStyleActionSheet];
    [alert addAction:[UIAlertAction actionWithTitle:@"取消" style:UIAlertActionStyleCancel handler:nil]];
    [alert addAction:[UIAlertAction actionWithTitle:@"清除" style:UIAlertActionStyleDestructive
        handler:^(UIAlertAction *action) {
            [[NSFileManager defaultManager] removeItemAtPath:NCLRStore.shared.configPath error:nil];
            [[NSFileManager defaultManager] removeItemAtPath:NCLRStore.shared.modsDirectory error:nil];
            [[NSFileManager defaultManager] removeItemAtPath:NCLRStore.shared.versionsDirectory error:nil];
            [NCLRStore.shared.versions removeAllObjects];
            [NCLRStore.shared.mods removeAllObjects];
            [NCLRStore.shared save];
            if (self.onDataChanged) self.onDataChanged();
        }]];
    alert.popoverPresentationController.sourceView = self.view;
    alert.popoverPresentationController.sourceRect = CGRectMake(CGRectGetMidX(self.view.bounds),
                                                                 CGRectGetMidY(self.view.bounds), 1, 1);
    [self presentViewController:alert animated:YES completion:nil];
}
@end

@interface NCLRKeyCanvas : UIView
@property (nonatomic, copy) void (^layoutKeys)(void);
@end

@implementation NCLRKeyCanvas
- (void)layoutSubviews {
    [super layoutSubviews];
    if (self.layoutKeys) self.layoutKeys();
}
@end

@interface NCLRGridOverlay : UIView
@end
@implementation NCLRGridOverlay
- (void)drawRect:(CGRect)rect {
    CGContextRef ctx = UIGraphicsGetCurrentContext();
    CGContextSetStrokeColorWithColor(ctx, [NCLRBorder() colorWithAlphaComponent:0.55].CGColor);
    CGContextSetLineWidth(ctx, 0.5);
    for (NSInteger i = 1; i < 10; i++) {
        CGFloat x = CGRectGetWidth(rect) * i / 10.0;
        CGFloat y = CGRectGetHeight(rect) * i / 10.0;
        CGContextMoveToPoint(ctx, x, 0); CGContextAddLineToPoint(ctx, x, CGRectGetHeight(rect));
        CGContextMoveToPoint(ctx, 0, y); CGContextAddLineToPoint(ctx, CGRectGetWidth(rect), y);
    }
    CGContextStrokePath(ctx);
    CGContextSetStrokeColorWithColor(ctx, NCLRBorder().CGColor);
    CGContextMoveToPoint(ctx, CGRectGetMidX(rect), 0);
    CGContextAddLineToPoint(ctx, CGRectGetMidX(rect), CGRectGetHeight(rect));
    CGContextMoveToPoint(ctx, 0, CGRectGetMidY(rect));
    CGContextAddLineToPoint(ctx, CGRectGetWidth(rect), CGRectGetMidY(rect));
    CGContextStrokePath(ctx);
}
@end

@implementation NCLRKeymapController

- (void)viewDidLoad {
    [super viewDidLoad];
    self.view.backgroundColor = NCLRColor(0.02, 0.02, 0.02, 1.0);
    self.scroll.delaysContentTouches = NO;
    self.scroll.canCancelContentTouches = YES;
    self.keyButtons = [NSMutableArray array];
    self.visibilitySwitches = [NSMutableDictionary dictionary];
    self.previewButtons = [NSMutableDictionary dictionary];
    self.undoStack = [NSMutableArray array];
    self.redoStack = [NSMutableArray array];
    [self setupPage];
    self.view.backgroundColor = NCLRCanvas();
    self.stack.spacing = 12;
    [self.stack addArrangedSubview:[self headerWithTitle:@"触控编辑"
                                                subtitle:@"拖动按键调整位置，接入键鼠时自动隐藏触控键"
                                             actionTitle:nil action:nil]];

    UIView *menu = [self card];
    self.menuToggle = [self button:@"触控设置  ▾" action:@selector(toggleMenu) primary:NO];
    self.menuToggle.contentHorizontalAlignment = UIControlContentHorizontalAlignmentLeft;
    self.menuToggle.titleLabel.font = [UIFont systemFontOfSize:14.0 weight:UIFontWeightSemibold];
    self.sizePresetControl = [[UISegmentedControl alloc] initWithItems:@[@"小", @"标准", @"大"]];
    CGFloat currentSize = NCLRKeymap.shared.buttonSize;
    self.sizePresetControl.selectedSegmentIndex = currentSize <= 0.88 ? 0 : (currentSize >= 1.12 ? 2 : 1);
    [self.sizePresetControl addTarget:self action:@selector(sizePresetChanged:)
                     forControlEvents:UIControlEventValueChanged];

    UILabel *sizeLabel = [self label:@"按键大小" size:12.0 weight:UIFontWeightSemibold
                               color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    self.sizeSlider = [[UISlider alloc] init];
    self.sizeSlider.translatesAutoresizingMaskIntoConstraints = NO;
    self.sizeSlider.minimumValue = 0.5f;
    self.sizeSlider.maximumValue = 2.5f;
    self.sizeSlider.value = NCLRKeymap.shared.buttonSize;
    self.sizeSlider.minimumTrackTintColor = NCLRColor(0.95, 0.95, 0.95, 1.0);
    [self.sizeSlider addTarget:self action:@selector(sizeChanged:)
              forControlEvents:UIControlEventValueChanged];
    self.sizeValue = [self label:[NSString stringWithFormat:@"%.2fx", NCLRKeymap.shared.buttonSize]
                            size:11.0 weight:UIFontWeightSemibold
                           color:NCLRColor(0.95, 0.95, 0.95, 1.0)];
    self.sizeValue.textAlignment = NSTextAlignmentRight;
    self.sizeRow = [[UIStackView alloc] initWithArrangedSubviews:@[sizeLabel, self.sizeSlider, self.sizeValue]];
    self.sizeRow.axis = UILayoutConstraintAxisHorizontal;
    self.sizeRow.spacing = 10.0;
    self.sizeRow.alignment = UIStackViewAlignmentCenter;
    self.sizeRow.translatesAutoresizingMaskIntoConstraints = NO;
    [self.sizeValue.widthAnchor constraintEqualToConstant:48.0].active = YES;

    UILabel *opacityLabel = [self label:@"按钮透明度" size:12.0 weight:UIFontWeightSemibold
                                  color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    self.opacitySlider = [[UISlider alloc] init];
    self.opacitySlider.translatesAutoresizingMaskIntoConstraints = NO;
    self.opacitySlider.minimumValue = 0.15f;
    self.opacitySlider.maximumValue = 1.0f;
    self.opacitySlider.value = NCLRKeymap.shared.opacity;
    self.opacitySlider.minimumTrackTintColor = NCLRColor(0.95, 0.95, 0.95, 1.0);
    [self.opacitySlider addTarget:self action:@selector(opacityChanged:)
                 forControlEvents:UIControlEventValueChanged];
    self.opacityValue = [self label:[NSString stringWithFormat:@"%.0f%%", NCLRKeymap.shared.opacity * 100.0]
                                size:11.0 weight:UIFontWeightSemibold
                               color:NCLRColor(0.95, 0.95, 0.95, 1.0)];
    self.opacityValue.textAlignment = NSTextAlignmentRight;
    UIStackView *opacityRow = [[UIStackView alloc] initWithArrangedSubviews:
        @[opacityLabel, self.opacitySlider, self.opacityValue]];
    opacityRow.axis = UILayoutConstraintAxisHorizontal;
    opacityRow.spacing = 10.0;
    opacityRow.alignment = UIStackViewAlignmentCenter;
    opacityRow.translatesAutoresizingMaskIntoConstraints = NO;
    [self.opacityValue.widthAnchor constraintEqualToConstant:48.0].active = YES;

    self.modeControl = [[UISegmentedControl alloc] initWithItems:
        @[@"摇杆 + 准星", @"摇杆 + 点按", @"方向键"]];
    NSInteger modeIndex = [@[@"crosshair", @"tap", @"dpad"] indexOfObject:NCLRKeymap.shared.touchMode];
    self.modeControl.selectedSegmentIndex = modeIndex == NSNotFound ? 0 : modeIndex;
    [self.modeControl addTarget:self action:@selector(modeChanged:)
               forControlEvents:UIControlEventValueChanged];

    UILabel *dynamicLabel = [self label:@"动态摇杆" size:12.0 weight:UIFontWeightSemibold
                                  color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    self.dynamicSwitch = [[UISwitch alloc] init];
    self.dynamicSwitch.on = NCLRKeymap.shared.dynamicJoystick;
    [self.dynamicSwitch addTarget:self action:@selector(dynamicChanged:)
                 forControlEvents:UIControlEventValueChanged];
    UIView *dynamicSpacer = [[UIView alloc] init];
    UIStackView *dynamicRow = [[UIStackView alloc] initWithArrangedSubviews:
        @[dynamicLabel, dynamicSpacer, self.dynamicSwitch]];
    dynamicRow.axis = UILayoutConstraintAxisHorizontal;
    dynamicRow.alignment = UIStackViewAlignmentCenter;
    UILabel *leftLabel = [self label:@"左手模式" size:12.0 weight:UIFontWeightSemibold
                               color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    self.leftHandedSwitch = [[UISwitch alloc] init];
    self.leftHandedSwitch.on = NCLRKeymap.shared.leftHanded;
    [self.leftHandedSwitch addTarget:self action:@selector(leftHandedChanged:)
                    forControlEvents:UIControlEventValueChanged];
    UIView *leftSpacer = [[UIView alloc] init];
    UIStackView *leftRow = [[UIStackView alloc] initWithArrangedSubviews:
        @[leftLabel, leftSpacer, self.leftHandedSwitch]];
    leftRow.axis = UILayoutConstraintAxisHorizontal;
    leftRow.alignment = UIStackViewAlignmentCenter;

    UILabel *sprintLabel = [self label:@"摇杆冲刺" size:12.0 weight:UIFontWeightSemibold
                                 color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    self.joystickSprintSwitch = [[UISwitch alloc] init];
    self.joystickSprintSwitch.on = NCLRKeymap.shared.joystickSprint;
    [self.joystickSprintSwitch addTarget:self action:@selector(joystickSprintChanged:)
                        forControlEvents:UIControlEventValueChanged];
    UIView *sprintSpacer = [[UIView alloc] init];
    UIStackView *sprintRow = [[UIStackView alloc] initWithArrangedSubviews:
        @[sprintLabel, sprintSpacer, self.joystickSprintSwitch]];
    sprintRow.axis = UILayoutConstraintAxisHorizontal;
    sprintRow.alignment = UIStackViewAlignmentCenter;

    UILabel *sensitivityLabel = [self label:@"视角灵敏度" size:12.0 weight:UIFontWeightSemibold
                                      color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    self.sensitivitySlider = [[UISlider alloc] init];
    self.sensitivitySlider.minimumValue = 0.4f;
    self.sensitivitySlider.maximumValue = 2.5f;
    self.sensitivitySlider.value = NCLRKeymap.shared.sensitivity;
    self.sensitivitySlider.minimumTrackTintColor = NCLRColor(0.95, 0.95, 0.95, 1.0);
    [self.sensitivitySlider addTarget:self action:@selector(sensitivityChanged:)
                     forControlEvents:UIControlEventValueChanged];
    self.sensitivityValue = [self label:[NSString stringWithFormat:@"%.1fx", NCLRKeymap.shared.sensitivity]
                                  size:11.0 weight:UIFontWeightSemibold
                                 color:NCLRColor(0.95, 0.95, 0.95, 1.0)];
    self.sensitivityValue.textAlignment = NSTextAlignmentRight;
    UIStackView *sensitivityRow = [[UIStackView alloc] initWithArrangedSubviews:
        @[sensitivityLabel, self.sensitivitySlider, self.sensitivityValue]];
    sensitivityRow.axis = UILayoutConstraintAxisHorizontal;
    sensitivityRow.spacing = 10.0;
    sensitivityRow.alignment = UIStackViewAlignmentCenter;
    [self.sensitivityValue.widthAnchor constraintEqualToConstant:44.0].active = YES;
    UILabel *deadzoneLabel = [self label:@"摇杆死区" size:12.0 weight:UIFontWeightSemibold
                                    color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    self.deadzoneSlider = [[UISlider alloc] init];
    self.deadzoneSlider.minimumValue = 0.05f;
    self.deadzoneSlider.maximumValue = 0.35f;
    self.deadzoneSlider.value = NCLRKeymap.shared.joystickDeadzone;
    self.deadzoneSlider.minimumTrackTintColor = NCLRColor(0.95, 0.95, 0.95, 1.0);
    [self.deadzoneSlider addTarget:self action:@selector(deadzoneChanged:)
                  forControlEvents:UIControlEventValueChanged];
    self.deadzoneValue = [self label:[NSString stringWithFormat:@"%.0f%%", NCLRKeymap.shared.joystickDeadzone * 100.0]
                                  size:11.0 weight:UIFontWeightSemibold
                                 color:NCLRColor(0.95, 0.95, 0.95, 1.0)];
    self.deadzoneValue.textAlignment = NSTextAlignmentRight;
    UIStackView *deadzoneRow = [[UIStackView alloc] initWithArrangedSubviews:
        @[deadzoneLabel, self.deadzoneSlider, self.deadzoneValue]];
    deadzoneRow.axis = UILayoutConstraintAxisHorizontal;
    deadzoneRow.spacing = 10.0;
    deadzoneRow.alignment = UIStackViewAlignmentCenter;
    [self.deadzoneValue.widthAnchor constraintEqualToConstant:44.0].active = YES;

    UILabel *sprintButtonLabel = [self label:@"显示冲刺键" size:12.0 weight:UIFontWeightSemibold
                                        color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    self.sprintButtonSwitch = [[UISwitch alloc] init];
    self.sprintButtonSwitch.on = NCLRKeymap.shared.sprintButton;
    [self.sprintButtonSwitch addTarget:self action:@selector(sprintButtonChanged:)
                      forControlEvents:UIControlEventValueChanged];
    UIView *sprintButtonSpacer = [[UIView alloc] init];
    UIStackView *sprintButtonRow = [[UIStackView alloc] initWithArrangedSubviews:
        @[sprintButtonLabel, sprintButtonSpacer, self.sprintButtonSwitch]];
    sprintButtonRow.axis = UILayoutConstraintAxisHorizontal;
    sprintButtonRow.alignment = UIStackViewAlignmentCenter;
    UILabel *iconLabelsLabel = [self label:@"图标标签" size:12.0 weight:UIFontWeightSemibold
                                     color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    self.iconLabelsSwitch = [[UISwitch alloc] init];
    self.iconLabelsSwitch.on = NCLRKeymap.shared.iconLabels;
    [self.iconLabelsSwitch addTarget:self action:@selector(iconLabelsChanged:)
                    forControlEvents:UIControlEventValueChanged];
    UIView *iconLabelsSpacer = [[UIView alloc] init];
    UIStackView *iconLabelsRow = [[UIStackView alloc] initWithArrangedSubviews:
        @[iconLabelsLabel, iconLabelsSpacer, self.iconLabelsSwitch]];
    iconLabelsRow.axis = UILayoutConstraintAxisHorizontal;
    iconLabelsRow.alignment = UIStackViewAlignmentCenter;
    self.presetControl = [[UISegmentedControl alloc] initWithItems:@[@"FCL", @"经典", @"极简"]];
    self.presetControl.selectedSegmentIndex = 0;
    [self.presetControl addTarget:self action:@selector(presetChanged:)
                 forControlEvents:UIControlEventValueChanged];
    self.styleControl = [[UISegmentedControl alloc] initWithItems:@[@"圆盘", @"方框", @"极简"]];
    self.styleControl.selectedSegmentIndex = NCLRKeymap.shared.buttonStyle;
    [self.styleControl addTarget:self action:@selector(styleChanged:)
                forControlEvents:UIControlEventValueChanged];
    UIStackView *settings = [[UIStackView alloc] initWithArrangedSubviews:
        @[self.sizePresetControl, self.presetControl, self.styleControl, self.sizeRow, opacityRow,
          self.modeControl, dynamicRow, leftRow, sprintRow, sensitivityRow, deadzoneRow, sprintButtonRow, iconLabelsRow]];
    settings.axis = UILayoutConstraintAxisVertical;
    settings.spacing = 8.0;
    settings.hidden = YES;
    self.touchSettings = settings;
    UIStackView *menuStack = [[UIStackView alloc] initWithArrangedSubviews:@[self.menuToggle, settings]];
    menuStack.axis = UILayoutConstraintAxisVertical;
    menuStack.spacing = 8.0;
    menuStack.translatesAutoresizingMaskIntoConstraints = NO;
    [menu addSubview:menuStack];
    [NSLayoutConstraint activateConstraints:@[
        [menuStack.leadingAnchor constraintEqualToAnchor:menu.leadingAnchor constant:12.0],
        [menuStack.trailingAnchor constraintEqualToAnchor:menu.trailingAnchor constant:-12.0],
        [menuStack.topAnchor constraintEqualToAnchor:menu.topAnchor constant:10.0],
        [menuStack.bottomAnchor constraintEqualToAnchor:menu.bottomAnchor constant:-10.0],
    ]];
    UIView *previewCard = [self card];
    self.canvasStatus = [self label:@"触控布局" size:15 weight:UIFontWeightSemibold
                              color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    self.canvasStatus.lineBreakMode = NSLineBreakByTruncatingTail;
    self.undoTool = [self canvasTool:@"arrow.uturn.backward" label:@"撤销" action:@selector(undoTapped)];
    self.redoTool = [self canvasTool:@"arrow.uturn.forward" label:@"重做" action:@selector(redoTapped)];
    self.gridTool = [self canvasTool:@"grid" label:@"显示或隐藏网格" action:@selector(toggleGrid)];
    self.canvasClose = [self canvasTool:@"arrow.up.left.and.arrow.down.right" label:@"全屏编辑" action:@selector(toggleCanvas)];
    UIButton *more = [self canvasTool:@"ellipsis" label:@"更多布局操作" action:NULL];
    __weak typeof(self) weakEditor = self;
    more.menu = [UIMenu menuWithChildren:@[
        [UIAction actionWithTitle:@"添加触控键" image:[UIImage systemImageNamed:@"plus"] identifier:nil
                         handler:^(__kindof UIAction *action) { [weakEditor addKeyTapped]; }],
        [UIAction actionWithTitle:@"导入布局" image:[UIImage systemImageNamed:@"square.and.arrow.down"] identifier:nil
                         handler:^(__kindof UIAction *action) { [weakEditor importKeymap]; }],
        [UIAction actionWithTitle:@"导出布局" image:[UIImage systemImageNamed:@"square.and.arrow.up"] identifier:nil
                         handler:^(__kindof UIAction *action) { [weakEditor exportKeymap]; }],
        [UIAction actionWithTitle:@"重置布局" image:[UIImage systemImageNamed:@"arrow.counterclockwise"] identifier:nil
                         handler:^(__kindof UIAction *action) { [weakEditor resetLayoutTapped]; }],
    ]];
    more.showsMenuAsPrimaryAction = YES;
    UIStackView *toolbar = [[UIStackView alloc] initWithArrangedSubviews:
        @[self.canvasStatus, self.undoTool, self.redoTool, self.gridTool, self.canvasClose, more]];
    toolbar.alignment = UIStackViewAlignmentCenter;
    toolbar.spacing = 2;
    toolbar.translatesAutoresizingMaskIntoConstraints = NO;
    [self.canvasStatus setContentCompressionResistancePriority:UILayoutPriorityDefaultLow
                                                       forAxis:UILayoutConstraintAxisHorizontal];
    [self updateCanvasTools];
    NCLRKeyCanvas *keyCanvas = [[NCLRKeyCanvas alloc] init];
    __weak typeof(self) weakLayout = self;
    keyCanvas.layoutKeys = ^{ [weakLayout layoutPreviewKeys]; };
    self.preview = keyCanvas;
    self.preview.translatesAutoresizingMaskIntoConstraints = NO;
    self.preview.backgroundColor = NCLRCanvas();
    self.preview.layer.cornerRadius = 12.0;
    self.preview.layer.borderWidth = 1.0;
    self.preview.layer.borderColor = NCLRBorder().CGColor;
    self.preview.clipsToBounds = YES;
    self.layoutScreenControl = [[UISegmentedControl alloc] initWithItems:@[@"游戏内", @"背包", @"暂停"]];
    self.layoutScreenControl.translatesAutoresizingMaskIntoConstraints = NO;
    self.layoutScreenControl.selectedSegmentIndex = 0;
    self.layoutScreenControl.accessibilityLabel = @"选择触控布局界面";
    [self.layoutScreenControl addTarget:self action:@selector(layoutScreenChanged)
                       forControlEvents:UIControlEventValueChanged];
    [previewCard addSubview:toolbar];
    [previewCard addSubview:self.layoutScreenControl];
    [previewCard addSubview:self.preview];
    self.previewCard = previewCard;
    self.previewHeight = [self.preview.heightAnchor constraintEqualToConstant:200.0];
    [NSLayoutConstraint activateConstraints:@[
        [toolbar.leadingAnchor constraintEqualToAnchor:previewCard.leadingAnchor constant:12],
        [toolbar.trailingAnchor constraintEqualToAnchor:previewCard.trailingAnchor constant:-8],
        [toolbar.topAnchor constraintEqualToAnchor:previewCard.topAnchor constant:4],
        [self.preview.centerXAnchor constraintEqualToAnchor:previewCard.centerXAnchor],
        [self.layoutScreenControl.leadingAnchor constraintEqualToAnchor:toolbar.leadingAnchor],
        [self.layoutScreenControl.trailingAnchor constraintEqualToAnchor:toolbar.trailingAnchor],
        [self.layoutScreenControl.topAnchor constraintEqualToAnchor:toolbar.bottomAnchor constant:2],
        [self.preview.topAnchor constraintEqualToAnchor:self.layoutScreenControl.bottomAnchor constant:8],
        [self.preview.widthAnchor constraintLessThanOrEqualToAnchor:previewCard.widthAnchor constant:-24.0],
        [self.preview.bottomAnchor constraintEqualToAnchor:previewCard.bottomAnchor constant:-10.0],
        self.previewHeight,
    ]];
    NSLayoutConstraint *previewAspect = [self.preview.widthAnchor constraintEqualToAnchor:self.preview.heightAnchor multiplier:16.0 / 9.0];
    previewAspect.priority = 999.0;
    previewAspect.active = YES;
    self.grid = [[NCLRGridOverlay alloc] initWithFrame:self.preview.bounds];
    self.grid.autoresizingMask = UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
    self.grid.userInteractionEnabled = NO;
    self.grid.backgroundColor = UIColor.clearColor;
    [self.preview addSubview:self.grid];
    [self updateCanvasTools];

    self.selection = [[UIView alloc] initWithFrame:CGRectZero];
    self.selection.userInteractionEnabled = NO;
    self.selection.backgroundColor = UIColor.clearColor;
    self.selection.layer.borderWidth = 2.0;
    self.selection.layer.borderColor = NCLRAccent().CGColor;
    self.selection.layer.cornerRadius = 10.0;
    self.selection.hidden = YES;
    [self.preview addSubview:self.selection];

    self.resizeHandle = [[UIView alloc] initWithFrame:CGRectMake(0, 0, 24, 24)];
    self.resizeHandle.backgroundColor = NCLRAccent();
    self.resizeHandle.layer.cornerRadius = 12.0;
    self.resizeHandle.layer.borderWidth = 2.0;
    self.resizeHandle.layer.borderColor = NCLRColor(0.05, 0.05, 0.05, 0.9).CGColor;
    self.resizeHandle.hidden = YES;
    self.resizeHandle.userInteractionEnabled = YES;
    [self.resizeHandle addGestureRecognizer:[[UIPanGestureRecognizer alloc] initWithTarget:self
                                                                                   action:@selector(resizeKey:)]];
    [self.preview addSubview:self.resizeHandle];

    [self refreshPreviewButtons];
    UIView *keyCard = [self card];
    self.keyEditTitle = [self label:@"选择画布上的按键"
                               size:13.0 weight:UIFontWeightSemibold
                              color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    UILabel *keySizeLabel = [self label:@"高度" size:12.0 weight:UIFontWeightSemibold
                                  color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    UILabel *keyWidthLabel = [self label:@"宽度" size:12.0 weight:UIFontWeightSemibold
                                   color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    self.keyWidthSlider = [[UISlider alloc] init];
    self.keyWidthSlider.minimumValue = 0.045f;
    self.keyWidthSlider.maximumValue = 1.0f;
    self.keyWidthSlider.value = 0.12f;
    [self.keyWidthSlider addTarget:self action:@selector(keySizeEditingStarted)
                  forControlEvents:UIControlEventTouchDown];
    [self.keyWidthSlider addTarget:self action:@selector(keySizeEditingFinished)
                  forControlEvents:UIControlEventTouchUpInside | UIControlEventTouchUpOutside | UIControlEventTouchCancel];
    self.keyWidthSlider.minimumTrackTintColor = NCLRColor(0.95, 0.95, 0.95, 1.0);
    [self.keyWidthSlider addTarget:self action:@selector(keyWidthChanged:)
                  forControlEvents:UIControlEventValueChanged];
    self.keyWidthValue = [self label:@"0.12" size:11.0 weight:UIFontWeightSemibold
                             color:NCLRColor(0.95, 0.95, 0.95, 1.0)];
    self.keyWidthSlider.enabled = NO;
    self.keyWidthValue.textAlignment = NSTextAlignmentRight;
    [self.keyWidthValue.widthAnchor constraintEqualToConstant:48.0].active = YES;
    UIStackView *keyWidthRow = [[UIStackView alloc] initWithArrangedSubviews:
        @[keyWidthLabel, self.keyWidthSlider, self.keyWidthValue]];
    keyWidthRow.axis = UILayoutConstraintAxisHorizontal;
    keyWidthRow.spacing = 10.0;
    keyWidthRow.alignment = UIStackViewAlignmentCenter;
    self.keySizeSlider = [[UISlider alloc] init];
    self.keySizeSlider.minimumValue = 0.045f;
    self.keySizeSlider.maximumValue = 0.45f;
    self.keySizeSlider.value = 0.12f;
    [self.keySizeSlider addTarget:self action:@selector(keySizeEditingStarted)
                  forControlEvents:UIControlEventTouchDown];
    [self.keySizeSlider addTarget:self action:@selector(keySizeEditingFinished)
                  forControlEvents:UIControlEventTouchUpInside | UIControlEventTouchUpOutside | UIControlEventTouchCancel];
    self.keySizeSlider.minimumTrackTintColor = NCLRColor(0.95, 0.95, 0.95, 1.0);
    [self.keySizeSlider addTarget:self action:@selector(keySizeChanged:)
                  forControlEvents:UIControlEventValueChanged];
    self.keySizeValue = [self label:@"0.12" size:11.0 weight:UIFontWeightSemibold
                             color:NCLRColor(0.95, 0.95, 0.95, 1.0)];
    self.keySizeSlider.enabled = NO;
    self.keySizeValue.textAlignment = NSTextAlignmentRight;
    [self.keySizeValue.widthAnchor constraintEqualToConstant:48.0].active = YES;
    UIStackView *keySizeRow = [[UIStackView alloc] initWithArrangedSubviews:
        @[keySizeLabel, self.keySizeSlider, self.keySizeValue]];
    keySizeRow.axis = UILayoutConstraintAxisHorizontal;
    keySizeRow.spacing = 10.0;
    keySizeRow.alignment = UIStackViewAlignmentCenter;
    UILabel *keyVisibleLabel = [self label:@"显示" size:12.0 weight:UIFontWeightSemibold
                                    color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    self.keyVisibleSwitch = [[UISwitch alloc] init];
    self.keyVisibleSwitch.enabled = NO;
    self.keyVisibleSwitch.accessibilityLabel = @"在游戏中显示此按键";
    [self.keyVisibleSwitch addTarget:self action:@selector(keyVisibleChanged:)
                     forControlEvents:UIControlEventValueChanged];
    UIStackView *keyVisibleRow = [[UIStackView alloc] initWithArrangedSubviews:
        @[keyVisibleLabel, self.keyVisibleSwitch]];
    keyVisibleRow.axis = UILayoutConstraintAxisHorizontal;
    keyVisibleRow.alignment = UIStackViewAlignmentCenter;
    keyVisibleRow.spacing = 6;
    self.triggerControl = [[UISegmentedControl alloc] initWithItems:@[@"点按", @"切换"]];
    self.triggerControl.accessibilityLabel = @"按键触发方式";
    self.triggerControl.enabled = NO;
    self.triggerControl.selectedSegmentIndex = 0;
    [self.triggerControl addTarget:self action:@selector(triggerChanged:) forControlEvents:UIControlEventValueChanged];
    UILabel *inspectorTitle = [self eyebrow:@"选中按键"];
    inspectorTitle.textColor = NCLRAccent();
    self.keyEditTitle.numberOfLines = 1;
    [self.keyEditTitle setContentCompressionResistancePriority:UILayoutPriorityDefaultLow
                                                      forAxis:UILayoutConstraintAxisHorizontal];
    self.deleteKeyButton = [self button:@"删除按键" action:@selector(deleteSelectedKey) primary:NO];
    self.deleteKeyButton.enabled = NO;
    [self.deleteKeyButton setTitleColor:NCLRDarkMode ? [UIColor colorWithRed:1 green:0.57 blue:0.57 alpha:1]
                                                   : [UIColor colorWithRed:0.62 green:0.12 blue:0.17 alpha:1]
                               forState:UIControlStateNormal];
    self.keyCoordinates = [self label:@"X —   Y —" size:12 weight:UIFontWeightMedium color:NCLRAccent()];
    self.keyCoordinates.font = [UIFont monospacedDigitSystemFontOfSize:12 weight:UIFontWeightMedium];
    NSMutableArray<UIView *> *inspector = [@[inspectorTitle, self.keyEditTitle, self.keyCoordinates] mutableCopy];
    for (NSString *axis in @[@"X", @"Y"]) {
        UISlider *slider = [[UISlider alloc] init];
        slider.minimumValue = 0.025;
        slider.maximumValue = 0.975;
        slider.enabled = NO;
        slider.accessibilityLabel = [axis stringByAppendingString:@" 坐标"];
        slider.minimumTrackTintColor = NCLRAccent();
        [slider addTarget:self action:@selector(keySizeEditingStarted) forControlEvents:UIControlEventTouchDown];
        [slider addTarget:self action:@selector(keySizeEditingFinished)
         forControlEvents:UIControlEventTouchUpInside | UIControlEventTouchUpOutside | UIControlEventTouchCancel];
        [slider addTarget:self action:@selector(keyCoordinateChanged:) forControlEvents:UIControlEventValueChanged];
        if ([axis isEqualToString:@"X"]) self.keyXSlider = slider; else self.keyYSlider = slider;
        UILabel *axisLabel = [self label:axis size:12 weight:UIFontWeightSemibold color:NCLRAccent()];
        UIStackView *row = [[UIStackView alloc] initWithArrangedSubviews:@[axisLabel, slider]];
        row.alignment = UIStackViewAlignmentCenter;
        row.spacing = 10;
        [inspector addObject:row];
    }
    [inspector addObjectsFromArray:@[keyWidthRow, keySizeRow, keyVisibleRow, self.triggerControl, self.deleteKeyButton]];
    UIStackView *keyStack = [[UIStackView alloc] initWithArrangedSubviews:inspector];
    keyStack.axis = UILayoutConstraintAxisVertical;
    keyStack.spacing = 9.0;
    keyStack.translatesAutoresizingMaskIntoConstraints = NO;
    [keyCard addSubview:keyStack];
    [NSLayoutConstraint activateConstraints:@[
        [keyStack.leadingAnchor constraintEqualToAnchor:keyCard.leadingAnchor constant:16.0],
        [keyStack.trailingAnchor constraintEqualToAnchor:keyCard.trailingAnchor constant:-16.0],
        [keyStack.topAnchor constraintEqualToAnchor:keyCard.topAnchor constant:14.0],
        [keyStack.bottomAnchor constraintEqualToAnchor:keyCard.bottomAnchor constant:-14.0],
    ]];
    self.editorInspector = keyCard;
    self.inspectorWidth = [keyCard.widthAnchor constraintEqualToConstant:222];
    self.editorWorkspace = [[UIStackView alloc] initWithArrangedSubviews:@[previewCard, keyCard]];
    self.editorWorkspace.axis = UILayoutConstraintAxisVertical;
    self.editorWorkspace.spacing = 12;
    [self.stack addArrangedSubview:self.editorWorkspace];
    [self.stack addArrangedSubview:[self gyroCard]];
    [self.stack addArrangedSubview:menu];

    UIButton *save = [self button:@"保存" action:@selector(saveKeymap) primary:YES];
    UIButton *exit = [self button:@"退出" action:@selector(exitKeymap) primary:NO];
    UIStackView *actions = [[UIStackView alloc] initWithArrangedSubviews:@[save, exit]];
    actions.axis = UILayoutConstraintAxisHorizontal;
    actions.spacing = 10.0;
    actions.distribution = UIStackViewDistributionFillEqually;
    [self.stack addArrangedSubview:actions];
}

- (void)refreshPreviewButtons {
    NSMutableArray<NSString *> *canvasKeys = [NCLRKeymap.shared.bindings.allKeys mutableCopy];
    for (NSDictionary *item in NCLRKeymap.shared.menuKeys) {
        NSString *identifier = item[@"id"];
        if (identifier.length > 0) [canvasKeys addObject:identifier];
    }
    for (NSString *key in canvasKeys) {
        if (self.previewButtons[key] || [key isEqualToString:@"forward"] || [key isEqualToString:@"back"] ||
            [key isEqualToString:@"left"] || [key isEqualToString:@"right"]) {
            continue;
        }
        UIButton *button = [UIButton buttonWithType:UIButtonTypeSystem];
        button.accessibilityIdentifier = key;
        BOOL move = [key isEqualToString:@"move"];
        if (move) {
            [button setTitle:nil forState:UIControlStateNormal];
            button.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.08];
            button.layer.borderWidth = 2.0;
            button.layer.borderColor = [UIColor colorWithWhite:1.0 alpha:0.35].CGColor;
            UIView *knob = [[UIView alloc] initWithFrame:CGRectZero];
            knob.tag = 991;
            knob.userInteractionEnabled = NO;
            knob.backgroundColor = NCLRAccent();
            knob.layer.cornerRadius = 8.0;
            [button addSubview:knob];
            UILabel *caption = [self label:@"移动" size:9.0 weight:UIFontWeightSemibold
                                         color:NCLRColor(0.92, 0.92, 0.92, 1.0)];
            caption.tag = 992;
            caption.textAlignment = NSTextAlignmentCenter;
            caption.userInteractionEnabled = NO;
            caption.translatesAutoresizingMaskIntoConstraints = YES;
            [button addSubview:caption];
        } else {
            NSString *binding = [NCLRKeymap.shared labelForCanvasKey:key];
            button.accessibilityLabel = binding;
            NSString *caption = [binding isEqualToString:@"MouseLeft"] ? @"左键"
                : [binding isEqualToString:@"MouseRight"] ? @"右键"
                : [binding isEqualToString:@"MouseMiddle"] ? @"中键"
                : [binding isEqualToString:@"Space"] ? @"空格" : binding;
            [button setTitle:caption forState:UIControlStateNormal];
            [button setTitleColor:NCLRColor(0.04, 0.04, 0.04, 1.0) forState:UIControlStateNormal];
            button.titleLabel.font = [UIFont monospacedSystemFontOfSize:11.0 weight:UIFontWeightBold];
            button.backgroundColor = NCLRColor(0.95, 0.95, 0.95, 0.86);
            button.layer.cornerRadius = 8.0;
        }
        [button addGestureRecognizer:[[UIPanGestureRecognizer alloc] initWithTarget:self
                                                                            action:@selector(dragKey:)]];
        [button addGestureRecognizer:[[UITapGestureRecognizer alloc] initWithTarget:self
                                                                              action:@selector(tapKey:)]];
        [self.preview addSubview:button];
        self.previewButtons[key] = button;
    }
    [self.preview bringSubviewToFront:self.selection];
    [self.preview bringSubviewToFront:self.resizeHandle];
    [self.preview setNeedsLayout];
}

- (UIView *)gyroCard {
    UIView *card = [self card];
    UILabel *title = [self label:@"陀螺仪视角" size:15 weight:UIFontWeightSemibold
                            color:NCLRColor(0.94, 0.94, 0.94, 1)];
    UIImageView *icon = [[UIImageView alloc] initWithImage:[UIImage systemImageNamed:@"gyroscope"]];
    icon.tintColor = NCLRAccent();
    icon.contentMode = UIViewContentModeScaleAspectFit;
    UISwitch *enabled = [[UISwitch alloc] init];
    enabled.onTintColor = NCLRAccent();
    enabled.on = NCLRKeymap.shared.gyroEnabled;
    enabled.accessibilityLabel = @"启用陀螺仪视角";
    [enabled addTarget:self action:@selector(gyroToggled:) forControlEvents:UIControlEventValueChanged];
    UIView *spacer = [UIView new];
    UIStackView *header = [[UIStackView alloc] initWithArrangedSubviews:@[icon, title, spacer, enabled]];
    header.alignment = UIStackViewAlignmentCenter;
    header.spacing = 10;
    self.gyroSlider = [[UISlider alloc] init];
    self.gyroSlider.minimumValue = 0.25;
    self.gyroSlider.maximumValue = 2.5;
    self.gyroSlider.value = NCLRKeymap.shared.gyroSensitivity;
    self.gyroSlider.enabled = enabled.on;
    self.gyroSlider.minimumTrackTintColor = NCLRAccent();
    self.gyroSlider.accessibilityLabel = @"陀螺仪灵敏度";
    [self.gyroSlider addTarget:self action:@selector(gyroSensitivityChanged:) forControlEvents:UIControlEventValueChanged];
    UILabel *caption = [self label:@"灵敏度" size:12 weight:UIFontWeightMedium color:NCLRColor(0.76,0.76,0.76,1)];
    self.gyroValue = [self label:[NSString stringWithFormat:@"%.2f×", self.gyroSlider.value]
                           size:12 weight:UIFontWeightSemibold color:NCLRAccent()];
    self.gyroValue.font = [UIFont monospacedDigitSystemFontOfSize:12 weight:UIFontWeightSemibold];
    UIStackView *row = [[UIStackView alloc] initWithArrangedSubviews:@[caption, self.gyroSlider, self.gyroValue]];
    row.alignment = UIStackViewAlignmentCenter;
    row.spacing = 10;
    self.gyroInvertXSwitch = [self gyroAxisSwitch:NCLRKeymap.shared.gyroInvertX
                                          label:@"反转 X 轴（左右）" enabled:enabled.on];
    self.gyroInvertYSwitch = [self gyroAxisSwitch:NCLRKeymap.shared.gyroInvertY
                                          label:@"反转 Y 轴（上下）" enabled:enabled.on];
    UIStackView *invertXRow = [self gyroAxisRow:self.gyroInvertXSwitch];
    UIStackView *invertYRow = [self gyroAxisRow:self.gyroInvertYSwitch];
    UIStackView *content = [[UIStackView alloc] initWithArrangedSubviews:@[header, row, invertXRow, invertYRow]];
    content.axis = UILayoutConstraintAxisVertical;
    content.spacing = 5;
    content.translatesAutoresizingMaskIntoConstraints = NO;
    [card addSubview:content];
    [NSLayoutConstraint activateConstraints:@[
        [icon.widthAnchor constraintEqualToConstant:22],
        [icon.heightAnchor constraintEqualToConstant:22],
        [self.gyroValue.widthAnchor constraintEqualToConstant:42],
        [content.leadingAnchor constraintEqualToAnchor:card.leadingAnchor constant:14],
        [content.trailingAnchor constraintEqualToAnchor:card.trailingAnchor constant:-14],
        [content.topAnchor constraintEqualToAnchor:card.topAnchor constant:10],
        [content.bottomAnchor constraintEqualToAnchor:card.bottomAnchor constant:-10],
    ]];
    CMMotionManager *motion = [CMMotionManager new];
    if (!motion.gyroAvailable) {
        enabled.enabled = NO;
        self.gyroSlider.enabled = NO;
        self.gyroInvertXSwitch.enabled = NO;
        self.gyroInvertYSwitch.enabled = NO;
        title.text = @"此设备不支持陀螺仪";
    }
    return card;
}

- (UISwitch *)gyroAxisSwitch:(BOOL)on label:(NSString *)label enabled:(BOOL)enabled {
    UISwitch *toggle = [[UISwitch alloc] init];
    toggle.on = on;
    toggle.enabled = enabled;
    toggle.onTintColor = NCLRAccent();
    toggle.accessibilityLabel = label;
    [toggle addTarget:self action:@selector(gyroAxesChanged) forControlEvents:UIControlEventValueChanged];
    return toggle;
}

- (UIStackView *)gyroAxisRow:(UISwitch *)toggle {
    UILabel *label = [self label:toggle.accessibilityLabel size:12 weight:UIFontWeightMedium
                            color:NCLRColor(0.76, 0.76, 0.76, 1)];
    label.isAccessibilityElement = NO;
    UIStackView *row = [[UIStackView alloc] initWithArrangedSubviews:@[label, [UIView new], toggle]];
    row.alignment = UIStackViewAlignmentCenter;
    row.spacing = 10;
    return row;
}

- (void)gyroAxesChanged {
    NCLRKeymap *keymap = NCLRKeymap.shared;
    keymap.gyroInvertX = self.gyroInvertXSwitch.on;
    keymap.gyroInvertY = self.gyroInvertYSwitch.on;
    [keymap save];
}

- (void)gyroToggled:(UISwitch *)sender {
    NCLRKeymap.shared.gyroEnabled = sender.on;
    self.gyroSlider.enabled = sender.on;
    self.gyroInvertXSwitch.enabled = sender.on;
    self.gyroInvertYSwitch.enabled = sender.on;
    [NCLRKeymap.shared save];
}

- (void)gyroSensitivityChanged:(UISlider *)sender {
    self.gyroValue.text = [NSString stringWithFormat:@"%.2f×", sender.value];
    NCLRKeymap.shared.gyroSensitivity = sender.value;
    [NCLRKeymap.shared save];
}

- (void)viewDidLayoutSubviews {
    [super viewDidLayoutSubviews];
    BOOL wide = !self.canvasFullscreen && CGRectGetWidth(self.view.bounds) >= 720;
    UILayoutConstraintAxis axis = wide ? UILayoutConstraintAxisHorizontal : UILayoutConstraintAxisVertical;
    if (self.editorWorkspace.axis != axis) {
        self.inspectorWidth.active = NO;
        self.editorWorkspace.axis = axis;
        self.inspectorWidth.active = wide;
    }
}

- (UIButton *)canvasTool:(NSString *)symbol label:(NSString *)label action:(SEL)action {
    UIButton *button = [UIButton buttonWithType:UIButtonTypeSystem];
    button.translatesAutoresizingMaskIntoConstraints = NO;
    button.tintColor = NCLRColor(0.94, 0.94, 0.94, 1.0);
    button.accessibilityLabel = label;
    [button setImage:[UIImage systemImageNamed:symbol] forState:UIControlStateNormal];
    [NSLayoutConstraint activateConstraints:@[
        [button.widthAnchor constraintEqualToConstant:44],
        [button.heightAnchor constraintEqualToConstant:44],
    ]];
    if (action) [button addTarget:self action:action forControlEvents:UIControlEventTouchUpInside];
    return button;
}

- (void)updateCanvasTools {
    self.undoTool.enabled = self.undoStack.count > 0;
    self.redoTool.enabled = self.redoStack.count > 0;
    self.gridTool.tintColor = self.grid.hidden ? NCLRColor(0.94, 0.94, 0.94, 1.0) : NCLRAccent();
    self.gridTool.accessibilityValue = self.grid.hidden ? @"已隐藏" : @"已显示";
}

- (void)toggleMenu {
    self.touchSettings.hidden = !self.touchSettings.hidden;
    [self.menuToggle setTitle:(self.touchSettings.hidden ? @"触控设置  ▾" : @"触控设置  ▴")
                     forState:UIControlStateNormal];
}

- (void)sizePresetChanged:(UISegmentedControl *)control {
    CGFloat sizes[3] = {0.85, 1.0, 1.15};
    self.sizeSlider.value = sizes[control.selectedSegmentIndex];
    [self sizeChanged:self.sizeSlider];
}

- (void)sizeChanged:(UISlider *)slider {
    self.sizeValue.text = [NSString stringWithFormat:@"%.2fx", slider.value];
    NCLRKeymap.shared.buttonSize = slider.value;
    for (UIButton *button in self.keyButtons) {
        button.titleLabel.font = [UIFont monospacedSystemFontOfSize:14.0 * slider.value
                                                              weight:UIFontWeightSemibold];
    }
    [self.preview setNeedsLayout];
}

- (BOOL)showsKeyOnCanvas:(NSString *)key {
    NSInteger screen = self.layoutScreenControl.selectedSegmentIndex;
    if (screen == 1 || screen == 2) {
        NSString *screenName = screen == 1 ? @"inventory" : @"pause";
        for (NSDictionary *item in NCLRKeymap.shared.menuKeys) {
            if ([item[@"id"] isEqual:key] && [item[@"screen"] isEqual:screenName]) return YES;
        }
        if (screen == 1) {
            return [key isEqualToString:NCLRInventoryEscape] ||
                   [key isEqualToString:NCLRInventoryKeyboard];
        }
        return [key isEqualToString:NCLRPauseEscape] ||
               [key isEqualToString:NCLRPauseKeyboard];
    }
    return !NCLRIsMenuKey(key) && ![NCLRKeymap.shared isMenuCanvasKey:key];
}

- (void)layoutScreenChanged {
    NSInteger screen = self.layoutScreenControl.selectedSegmentIndex;
    NSString *key = screen == 1 ? NCLRInventoryEscape : screen == 2 ? NCLRPauseEscape : nil;
    [self selectKey:key];
}

- (void)updateCoordinateReadout {
    NSDictionary *position = self.selectedKey ? NCLRKeymap.shared.positions[self.selectedKey] : nil;
    self.keyXSlider.enabled = position != nil;
    self.keyYSlider.enabled = position != nil;
    if (!position) {
        self.keyCoordinates.text = @"X —   Y —";
        return;
    }
    self.keyXSlider.value = [position[@"x"] doubleValue];
    self.keyYSlider.value = [position[@"y"] doubleValue];
    self.keyCoordinates.text = [NSString stringWithFormat:@"X %.1f%%   Y %.1f%%",
        100 * [position[@"x"] doubleValue], 100 * [position[@"y"] doubleValue]];
}

- (void)keyCoordinateChanged:(UISlider *)sender {
    if (!self.selectedKey.length) return;
    NSMutableDictionary *position = [NCLRKeymap.shared.positions[self.selectedKey] mutableCopy];
    if (!position) return;
    NSDictionary *before = sender.isTracking ? nil : [self keyLayoutSnapshot];
    position[sender == self.keyXSlider ? @"x" : @"y"] = @(sender.value);
    NCLRKeymap.shared.positions[self.selectedKey] = position;
    if (before) {
        [self pushUndo:before];
        [NCLRKeymap.shared save];
    }
    [self updateCoordinateReadout];
    [self.preview setNeedsLayout];
}

- (void)layoutPreviewKeys {
    if (self.preview.bounds.size.width <= 0.0 || self.preview.bounds.size.height <= 0.0) {
        return;
    }
    self.grid.frame = self.preview.bounds;
    for (NSString *key in self.previewButtons) {
        UIButton *button = self.previewButtons[key];
        button.hidden = ![self showsKeyOnCanvas:key] ||
            [NCLRKeymap.shared.deletedKeys containsObject:key] || !NCLRKeymap.shared.bindings[key];
        if (button.hidden) continue;
        NSDictionary *position = key.length ? NCLRKeymap.shared.positions[key] : nil;
        if (position == nil) position = @{ @"x" : @0.5, @"y" : @0.5, @"size" : @0.08, @"width" : @0.08 };
        CGFloat x = [position[@"x"] doubleValue];
        CGFloat y = [position[@"y"] doubleValue];
        CGRect area = [self previewContentRect];
        BOOL menuKey = NCLRIsMenuKey(key);
        CGFloat scale = menuKey ? 1.0 : NCLRKeymap.shared.buttonSize;
        CGFloat size = [position[@"size"] doubleValue] * area.size.height * scale;
        CGFloat width = NCLRKeyWidthForPosition(position, key) * area.size.height * scale;
        size = MIN(area.size.height, MAX(menuKey ? 1.0 : 26.0, size));
        width = MIN(area.size.width, MAX(menuKey ? 1.0 : 26.0, width));
        CGFloat centerX = area.origin.x + x * area.size.width;
        CGFloat centerY = area.origin.y + y * area.size.height;
        centerX = MAX(CGRectGetMinX(area) + width / 2.0,
                      MIN(CGRectGetMaxX(area) - width / 2.0, centerX));
        centerY = MAX(CGRectGetMinY(area) + size / 2.0,
                      MIN(CGRectGetMaxY(area) - size / 2.0, centerY));
        button.frame = CGRectMake(centerX - width / 2.0, centerY - size / 2.0, width, size);
        button.titleLabel.font = [UIFont monospacedSystemFontOfSize:MAX(9.0, 11.0 * NCLRKeymap.shared.buttonSize)
                                                              weight:UIFontWeightBold];
        BOOL move = [key isEqualToString:@"move"];
        BOOL hidden = [NCLRKeymap.shared isHidden:key];
        BOOL selected = [key isEqualToString:self.selectedKey];
        button.alpha = 1.0;
        UIColor *keyInk = hidden || move ? NCLRColor(0.95, 0.95, 0.95, 1.0)
            : NCLRDarkMode ? UIColor.blackColor : UIColor.whiteColor;
        button.backgroundColor = hidden || move ? NCLRSurface() : NCLRAccent();
        [button setTitleColor:keyInk forState:UIControlStateNormal];
        button.titleLabel.adjustsFontSizeToFitWidth = YES;
        button.titleLabel.minimumScaleFactor = 0.65;
        button.accessibilityValue = hidden ? @"游戏中隐藏" : @"游戏中显示";
        button.layer.cornerRadius = move ? size / 2.0 : 8.0;
        button.layer.borderWidth = selected || move ? 2.0 : hidden ? 1.0 : 0.0;
        button.layer.borderColor = (selected ? NCLRAccent() : NCLRBorder()).CGColor;
        UIView *knob = [button viewWithTag:991];
        UILabel *caption = (UILabel *)[button viewWithTag:992];
        if (move) {
            CGFloat knobSize = MAX(18.0, size * 0.42);
            knob.frame = CGRectMake((size - knobSize) / 2.0, (size - knobSize) / 2.0 - 4.0, knobSize, knobSize);
            knob.layer.cornerRadius = knobSize / 2.0;
            caption.frame = CGRectMake(0.0, size - 18.0, size, 14.0);
        }
    }
    [self updateSelection];
}

- (void)selectKey:(NSString *)key {
    BOOL menuCanvasKey = [NCLRKeymap.shared isMenuCanvasKey:key];
    if (key && ([NCLRKeymap.shared.deletedKeys containsObject:key] ||
                (!NCLRKeymap.shared.bindings[key] && !menuCanvasKey))) key = nil;
    self.selectedKey = key;
    self.deleteKeyButton.enabled = key.length > 0 && (!NCLRIsMenuKey(key));
    self.triggerControl.enabled = key.length > 0 && !NCLRIsMenuKey(key) && !menuCanvasKey && ![key isEqualToString:@"move"];
    self.triggerControl.selectedSegmentIndex = key.length > 0 &&
        [NCLRKeymap.shared.triggerModes[key] isEqualToString:@"toggle"] ? 1 : 0;
    self.keySizeSlider.enabled = key.length > 0;
    self.keyVisibleSwitch.enabled = key.length > 0 && !NCLRIsMenuKey(key) && !menuCanvasKey;
    NSDictionary *position = key.length ? NCLRKeymap.shared.positions[key] : nil;
    CGFloat size = [position[@"size"] doubleValue];
    if (size <= 0.0) size = 0.12;
    CGFloat width = position ? NCLRKeyWidthForPosition(position, key) : 0.12;
    self.keySizeSlider.value = size;
    self.keySizeValue.text = [NSString stringWithFormat:@"%.2f", size];
    self.keyWidthSlider.value = width;
    self.keyWidthValue.text = [NSString stringWithFormat:@"%.2f", width];
    self.keyWidthSlider.enabled = key.length > 0;
    self.keyVisibleSwitch.on = key.length > 0 && ![NCLRKeymap.shared isHidden:key];
    NSString *caption = key.length ? [self.previewButtons[key] titleForState:UIControlStateNormal] : nil;
    self.keyEditTitle.text = key.length > 0 ? (caption ?: @"移动摇杆") : @"选择画布上的按键";
    self.canvasStatus.text = caption ?: @"触控布局";
    [self updateCoordinateReadout];
    [self updateCanvasTools];
    [self.preview setNeedsLayout];
}

- (NSDictionary *)keyLayoutSnapshot {
    NCLRKeymap *keymap = NCLRKeymap.shared;
    return @{@"positions": [keymap.positions copy], @"hidden": [keymap.hiddenKeys copy],
             @"bindings": [keymap.bindings copy], @"deletedKeys": [keymap.deletedKeys copy],
             @"menuKeys": [keymap.menuKeys mutableCopy],
             @"triggerModes": [keymap.triggerModes copy] ?: @{}};
}

- (void)deleteSelectedKey {
    NSString *key = self.selectedKey;
    if (!key.length || NCLRIsMenuKey(key)) return;
    NSDictionary *before = [self keyLayoutSnapshot];
    if ([NCLRKeymap.shared isMenuCanvasKey:key]) {
        NSMutableArray *kept = [NSMutableArray array];
        for (NSDictionary *item in NCLRKeymap.shared.menuKeys) {
            if (![item[@"id"] isEqual:key]) [kept addObject:item];
        }
        NCLRKeymap.shared.menuKeys = kept;
        [NCLRKeymap.shared.positions removeObjectForKey:key];
    } else {
        [NCLRKeymap.shared.deletedKeys addObject:key];
    }
    [self pushUndo:before];
    [NCLRKeymap.shared save];
    [self selectKey:nil];
}

- (void)restoreKeyLayoutSnapshot:(NSDictionary *)snapshot {
    NCLRKeymap *keys = NCLRKeymap.shared;
    keys.positions = [snapshot[@"positions"] mutableCopy];
    keys.hiddenKeys = [snapshot[@"hidden"] mutableCopy];
    keys.bindings = [snapshot[@"bindings"] mutableCopy];
    keys.deletedKeys = [snapshot[@"deletedKeys"] mutableCopy];
    keys.menuKeys = [snapshot[@"menuKeys"] mutableCopy] ?: [NSMutableArray array];
    keys.triggerModes = [snapshot[@"triggerModes"] mutableCopy];
    [keys save];
    [self refreshPreviewButtons];
    [self selectKey:self.selectedKey];
}

- (void)triggerChanged:(UISegmentedControl *)sender {
    if (!sender.enabled || self.selectedKey.length == 0) return;
    NCLRKeymap *keymap = NCLRKeymap.shared;
    NSDictionary *before = [self keyLayoutSnapshot];
    if (!keymap.triggerModes) keymap.triggerModes = [NSMutableDictionary dictionary];
    keymap.triggerModes[self.selectedKey] = sender.selectedSegmentIndex == 1 ? @"toggle" : @"tap";
    [self pushUndo:before];
    [keymap save];
}

- (void)keySizeEditingStarted {
    self.gestureStartPositions = [self keyLayoutSnapshot];
}

- (void)keySizeEditingFinished {
    [self pushUndo:self.gestureStartPositions];
    self.gestureStartPositions = nil;
    [NCLRKeymap.shared save];
}

- (void)keySizeChanged:(UISlider *)sender {
    if (self.selectedKey.length == 0) return;
    NSDictionary *old = NCLRKeymap.shared.positions[self.selectedKey];
    NCLRKeymap.shared.positions[self.selectedKey] = @{
        @"x" : old[@"x"] ?: @0.5,
        @"y" : old[@"y"] ?: @0.5,
        @"size" : @(sender.value),
        @"width" : @(NCLRKeyWidthForPosition(old, self.selectedKey)),
    };
    self.keySizeValue.text = [NSString stringWithFormat:@"%.2f", sender.value];
    if (!sender.isTracking) [NCLRKeymap.shared save];
    [self.preview setNeedsLayout];
}

- (void)keyWidthChanged:(UISlider *)sender {
    if (self.selectedKey.length == 0) return;
    NSDictionary *old = NCLRKeymap.shared.positions[self.selectedKey];
    NCLRKeymap.shared.positions[self.selectedKey] = @{
        @"x" : old[@"x"] ?: @0.5,
        @"y" : old[@"y"] ?: @0.5,
        @"size" : old[@"size"] ?: @0.12,
        @"width" : @(sender.value),
    };
    self.keyWidthValue.text = [NSString stringWithFormat:@"%.2f", sender.value];
    if (!sender.isTracking) [NCLRKeymap.shared save];
    [self.preview setNeedsLayout];
}

- (void)keyVisibleChanged:(UISwitch *)sender {
    if (self.selectedKey.length == 0) return;
    NSDictionary *before = [self keyLayoutSnapshot];
    [NCLRKeymap.shared setHidden:!sender.on forKey:self.selectedKey];
    [self pushUndo:before];
    [NCLRKeymap.shared save];
    [self.preview setNeedsLayout];
}

- (void)updateSelection {
    if (self.selectedKey.length == 0) {
        self.selection.hidden = YES;
        self.resizeHandle.hidden = YES;
        return;
    }
    UIButton *button = self.previewButtons[self.selectedKey];
    if (button == nil) {
        self.selection.hidden = YES;
        self.resizeHandle.hidden = YES;
        return;
    }
    CGRect frame = CGRectInset(button.frame, -3.0, -3.0);
    self.selection.frame = frame;
    self.selection.hidden = NO;
    self.resizeHandle.center = CGPointMake(CGRectGetMaxX(frame), CGRectGetMaxY(frame));
    self.resizeHandle.hidden = NO;
}

- (void)tapKey:(UITapGestureRecognizer *)sender {
    UIButton *button = (UIButton *)sender.view;
    [self selectKey:button.accessibilityIdentifier];
}

- (void)dragKey:(UIPanGestureRecognizer *)sender {
    UIButton *button = (UIButton *)sender.view;
    NSString *key = button.accessibilityIdentifier;
    if (key.length == 0) {
        return;
    }
    CGRect area = [self previewContentRect];
    if (CGRectIsEmpty(area)) return;
    if (sender.state == UIGestureRecognizerStateBegan) {
        [self selectKey:key];
        NSDictionary *savedPosition = NCLRKeymap.shared.positions[key];
        CGFloat x = [savedPosition[@"x"] doubleValue];
        CGFloat y = [savedPosition[@"y"] doubleValue];
        self.dragStartCenter = savedPosition
            ? CGPointMake(area.origin.x + x * area.size.width, area.origin.y + y * area.size.height)
            : button.center;
        self.gestureStartPositions = [self keyLayoutSnapshot];
        [[[UIImpactFeedbackGenerator alloc] initWithStyle:UIImpactFeedbackStyleLight] impactOccurred];
    }
    CGPoint translation = [sender translationInView:self.preview];
    CGFloat cx = self.dragStartCenter.x + translation.x;
    CGFloat cy = self.dragStartCenter.y + translation.y;
    CGFloat x = MAX(0.025, MIN(0.975, (cx - area.origin.x) / area.size.width));
    CGFloat y = MAX(0.025, MIN(0.975, (cy - area.origin.y) / area.size.height));
    // Light magnetic snapping: edges and the centres of other keys. It only
    // engages within 1.8% so free placement near a guide still works.
    if (x < 0.043) x = 0.025;
    if (x > 0.957) x = 0.975;
    if (y < 0.043) y = 0.025;
    if (y > 0.957) y = 0.975;
    for (NSString *other in NCLRKeymap.shared.positions) {
        if (![self showsKeyOnCanvas:other] || [other isEqualToString:key] || [NCLRKeymap.shared.deletedKeys containsObject:other]) continue;
        NSDictionary *p = NCLRKeymap.shared.positions[other];
        CGFloat ox = [p[@"x"] doubleValue], oy = [p[@"y"] doubleValue];
        if (fabs(x - ox) < 0.018) x = ox;
        if (fabs(y - oy) < 0.018) y = oy;
    }
    NSDictionary *old = NCLRKeymap.shared.positions[key];
    NCLRKeymap.shared.positions[key] = @{
        @"x" : @(x),
        @"y" : @(y),
        @"size" : old[@"size"] ?: @0.12,
        @"width" : @(NCLRKeyWidthForPosition(old, key)),
    };
    [self updateCoordinateReadout];
    [self.preview setNeedsLayout];
    [self.preview layoutIfNeeded];
    if (sender.state == UIGestureRecognizerStateEnded ||
        sender.state == UIGestureRecognizerStateCancelled) {
        [self pushUndo:self.gestureStartPositions];
        self.gestureStartPositions = nil;
        [NCLRKeymap.shared save];
        [[[UIImpactFeedbackGenerator alloc] initWithStyle:UIImpactFeedbackStyleMedium] impactOccurred];
        [self.preview setNeedsLayout];
    }
}

- (void)resizeKey:(UIPanGestureRecognizer *)sender {
    if (sender.state == UIGestureRecognizerStateBegan) {
        self.gestureStartPositions = [self keyLayoutSnapshot];
    }
    if (self.selectedKey.length == 0) {
        return;
    }
    UIButton *button = self.previewButtons[self.selectedKey];
    if (button == nil) {
        return;
    }
    CGPoint point = [sender locationInView:self.preview];
    CGFloat distance = NCLRIsMenuKey(self.selectedKey) ? fabs(point.y - button.center.y)
        : hypot(point.x - button.center.x, point.y - button.center.y);
    CGFloat size = MAX(NCLRIsMenuKey(self.selectedKey) ? 1.0 : 28.0, distance * 2.0);
    CGRect area = [self previewContentRect];
    CGFloat normalized = size / MAX(1.0, area.size.height * (NCLRIsMenuKey(self.selectedKey) ? 1.0 : NCLRKeymap.shared.buttonSize));
    normalized = MAX(0.045, MIN(0.45, normalized));
    NSDictionary *old = NCLRKeymap.shared.positions[self.selectedKey];
    CGFloat oldWidth = NCLRKeyWidthForPosition(old, self.selectedKey);
    CGFloat oldSize = [old[@"size"] doubleValue];
    CGFloat width = normalized * (oldSize > 0.0 ? oldWidth / oldSize : 1.0);
    width = MAX(0.045, MIN(1.0, width));
    NCLRKeymap.shared.positions[self.selectedKey] = @{
        @"x" : old[@"x"] ?: @0.5,
        @"y" : old[@"y"] ?: @0.5,
        @"size" : @(normalized),
        @"width" : @(width),
    };
    if (sender.state == UIGestureRecognizerStateEnded ||
        sender.state == UIGestureRecognizerStateCancelled) {
        [self pushUndo:self.gestureStartPositions];
        self.gestureStartPositions = nil;
        [NCLRKeymap.shared save];
        [[[UIImpactFeedbackGenerator alloc] initWithStyle:UIImpactFeedbackStyleMedium] impactOccurred];
    }
    [self.preview setNeedsLayout];
}

- (void)pushUndo:(NSDictionary *)snapshot {
    if (snapshot == nil || [snapshot isEqualToDictionary:[self keyLayoutSnapshot]]) return;
    [self.undoStack addObject:snapshot];
    if (self.undoStack.count > 30) [self.undoStack removeObjectAtIndex:0];
    [self.redoStack removeAllObjects];
    [self updateCanvasTools];
}

- (void)undoTapped {
    if (self.undoStack.count == 0) return;
    NSDictionary *previous = self.undoStack.lastObject;
    [self.undoStack removeLastObject];
    [self.redoStack addObject:[self keyLayoutSnapshot]];
    [self restoreKeyLayoutSnapshot:previous];
    [[[UIImpactFeedbackGenerator alloc] initWithStyle:UIImpactFeedbackStyleLight] impactOccurred];
}

- (void)redoTapped {
    if (self.redoStack.count == 0) return;
    NSDictionary *next = self.redoStack.lastObject;
    [self.redoStack removeLastObject];
    [self.undoStack addObject:[self keyLayoutSnapshot]];
    [self restoreKeyLayoutSnapshot:next];
    [[[UIImpactFeedbackGenerator alloc] initWithStyle:UIImpactFeedbackStyleLight] impactOccurred];
}

- (void)toggleGrid {
    self.grid.hidden = !self.grid.hidden;
    [self updateCanvasTools];
}

- (void)resetLayoutTapped {
    UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"重置布局"
                                                                   message:@"当前预设的位置和显隐会被恢复为默认值。"
                                                            preferredStyle:UIAlertControllerStyleAlert];
    [alert addAction:[UIAlertAction actionWithTitle:@"取消" style:UIAlertActionStyleCancel handler:nil]];
    [alert addAction:[UIAlertAction actionWithTitle:@"重置" style:UIAlertActionStyleDestructive
                                            handler:^(UIAlertAction *action) {
        NSDictionary *before = [self keyLayoutSnapshot];
        [NCLRKeymap.shared resetLayout];
        [self pushUndo:before];
        [NCLRKeymap.shared save];
        [self selectKey:self.selectedKey];
        [self.preview setNeedsLayout];
    }]];
    [self presentViewController:alert animated:YES completion:nil];
}

- (NSArray<NSString *> *)controlKeys {
    return @[@"move", @"jump", @"sneak", @"sprint", @"attack",
             @"use", @"inventory", @"chat", @"pause", @"settings",
             @"debug", @"perspective", @"offhand", @"shield", @"keyboard"];
}

- (void)deadzoneChanged:(UISlider *)slider {
    NCLRKeymap.shared.joystickDeadzone = slider.value;
    self.deadzoneValue.text = [NSString stringWithFormat:@"%.0f%%", slider.value * 100.0];
}

- (void)sprintButtonChanged:(UISwitch *)sender {
    NCLRKeymap.shared.sprintButton = sender.on;
}

- (void)leftHandedChanged:(UISwitch *)sender {
    NCLRKeymap.shared.leftHanded = sender.on;
}

- (void)joystickSprintChanged:(UISwitch *)sender {
    NCLRKeymap.shared.joystickSprint = sender.on;
}

- (void)sensitivityChanged:(UISlider *)slider {
    NCLRKeymap.shared.sensitivity = slider.value;
    self.sensitivityValue.text = [NSString stringWithFormat:@"%.1fx", slider.value];
}

- (void)iconLabelsChanged:(UISwitch *)sender {
    NCLRKeymap.shared.iconLabels = sender.on;
}

- (void)styleChanged:(UISegmentedControl *)control {
    NCLRKeymap.shared.buttonStyle = control.selectedSegmentIndex;
}

- (void)presetChanged:(UISegmentedControl *)control {
    NSArray<NSString *> *presets = @[@"fcl", @"classic", @"minimal"];
    NSInteger index = MAX(0, MIN((NSInteger)presets.count - 1, control.selectedSegmentIndex));
    [NCLRKeymap.shared applyPreset:presets[index]];
    [NCLRKeymap.shared save];
    for (NSString *key in self.visibilitySwitches) {
        self.visibilitySwitches[key].on = ![NCLRKeymap.shared isHidden:key];
    }
    [self.preview setNeedsLayout];
    [self.view layoutIfNeeded];
}

- (void)visibilityChanged:(UISwitch *)sender {
    NSArray<NSString *> *keys = [self controlKeys];
    if (sender.tag < 0 || sender.tag >= (NSInteger)keys.count) return;
    NSString *key = keys[sender.tag];
    [NCLRKeymap.shared setHidden:!sender.on forKey:key];
    [NCLRKeymap.shared save];
    UIButton *preview = self.previewButtons[key];
    preview.alpha = sender.on ? 1.0 : 0.22;
}

- (void)exportKeymap {
    [NCLRKeymap.shared save];
    NSURL *url = [NSURL fileURLWithPath:NCLRKeymap.shared.path];
    UIActivityViewController *share = [[UIActivityViewController alloc]
        initWithActivityItems:@[url] applicationActivities:nil];
    share.popoverPresentationController.sourceView = self.view;
    share.popoverPresentationController.sourceRect = CGRectMake(self.view.bounds.size.width / 2.0, 80.0, 1.0, 1.0);
    [self presentViewController:share animated:YES completion:nil];
}

- (void)importKeymap {
    UIDocumentPickerViewController *picker = [[UIDocumentPickerViewController alloc]
        initWithDocumentTypes:@[@"public.data"] inMode:UIDocumentPickerModeImport];
    picker.delegate = self;
    [self presentViewController:picker animated:YES completion:nil];
}

- (void)documentPicker:(UIDocumentPickerViewController *)controller
    didPickDocumentsAtURLs:(NSArray<NSURL *> *)urls {
    NSURL *url = urls.firstObject;
    if (url == nil) return;
    BOOL scoped = [url startAccessingSecurityScopedResource];
    NSString *path = NCLRKeymap.shared.path;
    [[NSFileManager defaultManager] removeItemAtPath:path error:nil];
    NSError *error = nil;
    BOOL ok = [[NSFileManager defaultManager] copyItemAtURL:url
                                                     toURL:[NSURL fileURLWithPath:path]
                                                     error:&error];
    if (scoped) [url stopAccessingSecurityScopedResource];
    if (ok) {
        [NCLRKeymap.shared load];
    }
    NSString *message = ok ? @"已导入，退出触控编辑再进入即可看到新布局。"
                           : (error.localizedDescription ?: @"无法导入");
    UIAlertController *alert = [UIAlertController alertControllerWithTitle:ok ? @"导入成功" : @"导入失败"
                                                                   message:message
                                                            preferredStyle:UIAlertControllerStyleAlert];
    [alert addAction:[UIAlertAction actionWithTitle:@"好" style:UIAlertActionStyleDefault handler:nil]];
    [self presentViewController:alert animated:YES completion:nil];
}

- (void)opacityChanged:(UISlider *)slider {
    self.opacityValue.text = [NSString stringWithFormat:@"%.0f%%", slider.value * 100.0];
}

- (void)modeChanged:(UISegmentedControl *)control {
}

- (void)dynamicChanged:(UISwitch *)sender {
}

- (void)addKeyTapped {
    [NCLRKeyPickerController presentFrom:self title:@"添加触控键" onPick:^(NSString *picked) {
        NSString *keyId = [picked isEqualToString:@"键盘"] ? @"keyboard" : picked;
        NSString *label = [keyId isEqualToString:@"keyboard"] ? @"系统键盘" : picked;
        NSDictionary *before = [self keyLayoutSnapshot];
        NSInteger screen = self.layoutScreenControl.selectedSegmentIndex;
        NSString *selected = keyId;
        if (screen == 1 || screen == 2) {
            selected = [NCLRKeymap.shared addMenuKey:keyId label:label screen:screen];
        } else {
            [NCLRKeymap.shared addKey:keyId label:label];
        }
        [self pushUndo:before];
        [NCLRKeymap.shared save];
        [self refreshPreviewButtons];
        [self selectKey:selected];
    }];
}
- (void)editKey:(UIButton *)sender {
    NSArray<NSString *> *keys = @[@"move", @"jump", @"sneak", @"sprint", @"attack",
                                  @"use", @"inventory", @"chat", @"pause", @"settings",
                                  @"debug", @"perspective", @"offhand", @"shield", @"keyboard"];
    NSArray<NSString *> *names = @[@"移动摇杆", @"跳跃", @"潜行", @"冲刺", @"攻击",
                                   @"使用", @"背包", @"聊天", @"暂停", @"触控设置",
                                   @"调试", @"视角", @"副手", @"盾牌", @"系统键盘"];
    if (sender.tag < 0 || sender.tag >= (NSInteger)keys.count) {
        return;
    }
    NSString *key = keys[sender.tag];
    __weak UIButton *weakSender = sender;
    [NCLRKeyPickerController presentFrom:self title:names[sender.tag] onPick:^(NSString *picked) {
        NCLRKeymap.shared.bindings[key] = picked;
        [weakSender setTitle:picked forState:UIControlStateNormal];
    }];
}
- (void)viewDidAppear:(BOOL)animated {
    [super viewDidAppear:animated];
    [UIView performWithoutAnimation:^{
        CGFloat height = self.view.bounds.size.height;
        self.previewHeight.constant = self.canvasFullscreen ? MAX(150.0, self.scroll.bounds.size.height - 110.0)
                                                            : MAX(170.0, MIN(410.0, height * 0.58));
        [self.view layoutIfNeeded];
    }];
}

- (BOOL)prefersStatusBarHidden {
    return YES;
}

- (BOOL)prefersHomeIndicatorAutoHidden {
    return YES;
}

- (CGRect)previewContentRect {
    if (!self.canvasFullscreen) return self.preview.bounds;
    return UIEdgeInsetsInsetRect(self.preview.bounds, UIEdgeInsetsMake(10.0, 10.0, 10.0, 10.0));
}

- (void)toggleCanvas {
    self.canvasFullscreen = !self.canvasFullscreen;
    for (UIView *view in self.stack.arrangedSubviews) {
        view.hidden = self.canvasFullscreen && view != self.editorWorkspace;
    }
    self.editorInspector.hidden = self.canvasFullscreen;
    if (self.canvasFullscreen) {
        self.inspectorWidth.active = NO;
        self.editorWorkspace.axis = UILayoutConstraintAxisVertical;
    }
    NSString *symbol = self.canvasFullscreen ? @"arrow.down.right.and.arrow.up.left" : @"arrow.up.left.and.arrow.down.right";
    [self.canvasClose setImage:[UIImage systemImageNamed:symbol] forState:UIControlStateNormal];
    self.canvasClose.accessibilityLabel = self.canvasFullscreen ? @"退出全屏" : @"全屏编辑";
    [self.preview setNeedsLayout];
    self.scroll.scrollEnabled = !self.canvasFullscreen;
    if (self.canvasFullscreen) [self.scroll setContentOffset:CGPointZero animated:NO];
    if (self.canvasFullscreen) {
        self.previewCard.backgroundColor = NCLRSurface();
        self.previewCard.layer.cornerRadius = 0.0;
        self.previewCard.layer.borderWidth = 0.0;
        self.preview.backgroundColor = NCLRCanvas();
        self.preview.layer.cornerRadius = 0.0;
        self.preview.layer.borderWidth = 0.0;
        self.stackLeading.constant = 0.0;
        self.stackTrailing.constant = 0.0;
        self.previewHeight.constant = MAX(150.0, self.scroll.bounds.size.height - 110.0);
    } else {
        self.previewCard.backgroundColor = NCLRSurface();
        self.previewCard.layer.cornerRadius = 16.0;
        self.previewCard.layer.borderWidth = 1.0;
        self.preview.backgroundColor = NCLRCanvas();
        self.preview.layer.cornerRadius = 12.0;
        self.preview.layer.borderWidth = 1.0;
        self.stackLeading.constant = 24.0;
        self.stackTrailing.constant = -24.0;
        self.previewHeight.constant = MAX(170.0, MIN(410.0, self.view.bounds.size.height * 0.58));
    }
    [self.preview setNeedsLayout];
    if (UIAccessibilityIsReduceMotionEnabled()) {
        [self.view layoutIfNeeded];
    } else {
        [UIView animateWithDuration:0.25 delay:0
                            options:UIViewAnimationOptionBeginFromCurrentState | UIViewAnimationOptionCurveEaseInOut
                         animations:^{ [self.view layoutIfNeeded]; } completion:nil];
    }
}
- (void)saveKeymap {
    NCLRKeymap.shared.buttonSize = self.sizeSlider.value;
    NCLRKeymap.shared.opacity = self.opacitySlider.value;
    NCLRKeymap.shared.touchMode = @[@"crosshair", @"tap", @"dpad"][MAX(0, self.modeControl.selectedSegmentIndex)];
    NCLRKeymap.shared.dynamicJoystick = self.dynamicSwitch.on;
    NCLRKeymap.shared.leftHanded = self.leftHandedSwitch.on;
    NCLRKeymap.shared.joystickSprint = self.joystickSprintSwitch.on;
    NCLRKeymap.shared.sensitivity = self.sensitivitySlider.value;
    NCLRKeymap.shared.joystickDeadzone = self.deadzoneSlider.value;
    NCLRKeymap.shared.sprintButton = self.sprintButtonSwitch.on;
    NCLRKeymap.shared.buttonStyle = self.styleControl.selectedSegmentIndex;
    NCLRKeymap.shared.iconLabels = self.iconLabelsSwitch.on;
    [NCLRKeymap.shared save];
    UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"已保存"
                                                                   message:[NSString stringWithFormat:
                                                                    @"触控界面已写入：\n%@", NCLRKeymap.shared.path]
                                                            preferredStyle:UIAlertControllerStyleAlert];
    [alert addAction:[UIAlertAction actionWithTitle:@"好" style:UIAlertActionStyleDefault handler:nil]];
    [self presentViewController:alert animated:YES completion:nil];
}

- (void)exitKeymap {
    [self dismissViewControllerAnimated:YES completion:nil];
}
@end

#pragma mark - Root

static NSString *NCLRSHA1Hex(NSData *data) {
    unsigned char digest[CC_SHA1_DIGEST_LENGTH];
    CC_SHA1(data.bytes, (CC_LONG)data.length, digest);
    NSMutableString *hex = [NSMutableString stringWithCapacity:CC_SHA1_DIGEST_LENGTH * 2];
    for (int i = 0; i < CC_SHA1_DIGEST_LENGTH; i++) [hex appendFormat:@"%02x", digest[i]];
    return hex;
}

/// First-run resource preparation: official jar and assets in, hzr-device out.
/// Runs on a background queue; progress strings go back to the main queue.
@implementation NCLRResourcePreparer {
    dispatch_queue_t _queue;
}
+ (instancetype)shared {
    static NCLRResourcePreparer *shared = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ shared = [[NCLRResourcePreparer alloc] init]; });
    return shared;
}
- (instancetype)init {
    self = [super init];
    if (self) _queue = dispatch_queue_create("dev.zrxmaxlnx.nclr.prepare", DISPATCH_QUEUE_SERIAL);
    return self;
}
- (void)cancel {
    if (!self.preparing || !self.cancellable || self.cancelRequested) return;
    @synchronized (self) {
        self.cancelRequested = YES;
        [self.downloadSession invalidateAndCancel];
    }
}

- (NSData *)download:(NSString *)url error:(NSString **)error {
    return [self download:url error:error received:nil];
}

- (NSData *)download:(NSString *)url error:(NSString **)error
             received:(void (^)(unsigned long long bytes))received {
    NSURL *address = [NSURL URLWithString:url];
    if (![address.scheme.lowercaseString isEqualToString:@"https"] || address.host.length == 0) {
        if (error) *error = @"下载地址无效";
        return nil;
    }
    __block NSString *failure = @"无响应";
    for (int attempt = 0; attempt < 3 && !self.cancelRequested; attempt++) {
        dispatch_semaphore_t finished = dispatch_semaphore_create(0);
        __block NSData *result = nil;
        NSURLSessionDataTask *task = nil;
        @synchronized (self) {
            if (self.cancelRequested) break;
            task = [self.downloadSession dataTaskWithURL:address
                completionHandler:^(NSData *data, NSURLResponse *response, NSError *networkError) {
                    NSInteger status = [(NSHTTPURLResponse *)response statusCode];
                    if (networkError == nil && status == 200) result = data;
                    else failure = networkError.localizedDescription ?: [NSString stringWithFormat:@"HTTP %ld", (long)status];
                    dispatch_semaphore_signal(finished);
                }];
            [task resume];
        }
        if (task == nil) break;
        if (received) {
            while (dispatch_semaphore_wait(finished, dispatch_time(DISPATCH_TIME_NOW, 120 * NSEC_PER_MSEC))) {
                received((unsigned long long)MAX(0, task.countOfBytesReceived));
            }
        } else {
            dispatch_semaphore_wait(finished, DISPATCH_TIME_FOREVER);
        }
        if (!self.cancelRequested && result != nil) return result;
        if (received) received(0);
    }
    if (error) *error = self.cancelRequested ? @"下载已取消"
        : [NSString stringWithFormat:@"下载失败：%@（%@）", address.lastPathComponent, failure];
    return nil;
}

- (NSData *)verifiedFile:(NSString *)path metadata:(NSDictionary *)metadata error:(NSString **)error {
    NSData *data = [NSData dataWithContentsOfFile:path options:NSDataReadingMappedIfSafe error:nil];
    NSString *hash = metadata[@"sha1"];
    if (data != nil && [NCLRSHA1Hex(data) isEqualToString:hash]) return data;
    data = [self download:metadata[@"url"] error:error];
    if (data == nil) return nil;
    if (![NCLRSHA1Hex(data) isEqualToString:hash]) {
        *error = [NSString stringWithFormat:@"%@ SHA-1 校验失败", path.lastPathComponent];
        return nil;
    }
    NSError *writeError = nil;
    if (![data writeToFile:path options:NSDataWritingAtomic error:&writeError]) {
        *error = writeError.localizedDescription;
        return nil;
    }
    return data;
}

- (void)prepareWithProgress:(void (^)(NSString *))progress
                 completion:(void (^)(NSString *))completion {
    if (self.preparing) return;
    self.preparing = YES;
    self.cancellable = YES;
    self.cancelRequested = NO;
    self.progressValue = 0;
    self.lastLine = @"准备游戏资源";
    NSURLSessionConfiguration *configuration = NSURLSessionConfiguration.ephemeralSessionConfiguration;
    configuration.URLCache = nil;
    configuration.timeoutIntervalForRequest = 20;
    configuration.timeoutIntervalForResource = 180;
    self.downloadSession = [NSURLSession sessionWithConfiguration:configuration];

    dispatch_async(_queue, ^{
        void (^finish)(NSString *) = ^(NSString *error) {
            @synchronized (self) {
                if (!self.cancelRequested) [self.downloadSession finishTasksAndInvalidate];
                self.downloadSession = nil;
            }
            dispatch_async(dispatch_get_main_queue(), ^{
                NSString *result = self.cancelRequested ? @"下载已取消" : error;
                self.cancellable = NO;
                self.preparing = NO;
                self.lastLine = result ?: @"资源准备完成";
                if (!result) self.progressValue = 1.0;
                [[NSNotificationCenter defaultCenter] postNotificationName:NCLRPrepareFinishedNotification
                                                                    object:self userInfo:@{@"status": self.lastLine}];
                if (completion) completion(result);
            });
        };
        void (^report)(NSString *, float) = ^(NSString *line, float value) {
            dispatch_async(dispatch_get_main_queue(), ^{
                if (self.cancelRequested) return;
                self.lastLine = line;
                self.progressValue = value;
                [[NSNotificationCenter defaultCenter] postNotificationName:NCLRPrepareProgressNotification
                                                                    object:self userInfo:@{@"line": line}];
                if (progress) progress(line);
            });
        };
        NSString *cache = [NCLRDocuments() stringByAppendingPathComponent:@".nclr-cache"];
        NSString *jarPath = [cache stringByAppendingPathComponent:@"client.jar"];
        NSString *indexPath = [cache stringByAppendingPathComponent:@"index.json"];
        NSString *objects = [cache stringByAppendingPathComponent:@"objects"];
        NSString *out = [NCLRDocuments() stringByAppendingPathComponent:@"horizon-client/26.3"];
        NCLRCreateDirectory(cache);
        NCLRCreateDirectory(objects);
        NSString *error = nil;

        report(@"读取 Mojang 版本清单…", 0.01);
        NSData *manifestData = [self download:@"https://piston-meta.mojang.com/mc/game/version_manifest_v2.json" error:&error];
        if (manifestData == nil) { finish(error); return; }
        id manifest = [NSJSONSerialization JSONObjectWithData:manifestData options:0 error:nil];
        id versions = [manifest isKindOfClass:NSDictionary.class] ? manifest[@"versions"] : nil;
        if (![versions isKindOfClass:NSArray.class]) { finish(@"Mojang 版本清单格式无效"); return; }
        NSString *versionURL = nil;
        for (id version in versions) {
            if ([version isKindOfClass:NSDictionary.class] &&
                [NCLRString(version[@"id"]) isEqualToString:@"26.3"]) {
                versionURL = NCLRString(version[@"url"]);
                break;
            }
        }
        if (versionURL == nil) { finish(@"Mojang 清单里没有 26.3"); return; }

        report(@"读取 26.3 版本信息…", 0.02);
        NSData *versionData = [self download:versionURL error:&error];
        if (versionData == nil) { finish(error); return; }
        id version = [NSJSONSerialization JSONObjectWithData:versionData options:0 error:nil];
        id downloads = [version isKindOfClass:NSDictionary.class] ? version[@"downloads"] : nil;
        id client = [downloads isKindOfClass:NSDictionary.class] ? downloads[@"client"] : nil;
        id assets = [version isKindOfClass:NSDictionary.class] ? version[@"assetIndex"] : nil;
        if (![client isKindOfClass:NSDictionary.class] || ![assets isKindOfClass:NSDictionary.class] ||
            !NCLRString(client[@"url"]) || !NCLRString(assets[@"url"]) ||
            !NCLRString(client[@"sha1"]) || !NCLRString(assets[@"sha1"])) {
            finish(@"版本信息缺少有效的 client/assetIndex");
            return;
        }

        report(@"检查 client.jar…", 0.03);
        if ([self verifiedFile:jarPath metadata:client error:&error] == nil) { finish(error); return; }

        report(@"检查资源索引…", 0.04);
        NSData *indexData = [self verifiedFile:indexPath metadata:assets error:&error];
        if (indexData == nil) { finish(error); return; }
        id index = [NSJSONSerialization JSONObjectWithData:indexData options:0 error:nil];
        NSDictionary *objectsMap = [index isKindOfClass:NSDictionary.class] ? index[@"objects"] : nil;
        if (![objectsMap isKindOfClass:NSDictionary.class] || objectsMap.count == 0) {
            finish(@"资源索引缺少资源列表");
            return;
        }
        NSArray<NSString *> *names = [objectsMap.allKeys sortedArrayUsingSelector:@selector(compare:)];
        NSMutableDictionary<NSString *, NSNumber *> *sizes = [NSMutableDictionary dictionary];
        NSCharacterSet *invalidHashCharacters = [[NSCharacterSet characterSetWithCharactersInString:@"0123456789abcdef"] invertedSet];
        for (NSString *name in names) {
            id object = objectsMap[name];
            NSString *hash = [object isKindOfClass:NSDictionary.class] ? NCLRString(object[@"hash"]) : nil;
            NSNumber *size = hash ? object[@"size"] : nil;
            if (hash.length != 40 || [hash rangeOfCharacterFromSet:invalidHashCharacters].location != NSNotFound ||
                ![size isKindOfClass:NSNumber.class] || size.longLongValue < 0 ||
                (sizes[hash] && ![sizes[hash] isEqualToNumber:size])) {
                finish(@"资源索引中的校验值或文件大小无效");
                return;
            }
            sizes[hash] = size;
        }

        NSOperationQueue *assetQueue = [[NSOperationQueue alloc] init];
        assetQueue.maxConcurrentOperationCount = 4;
        NSMutableArray<NSString *> *pending = [NSMutableArray array];
        unsigned long long totalBytes = 0;
        unsigned long long cachedBytes = 0;
        report(@"校验本地资源缓存…", 0.05);
        for (NSString *hash in sizes) {
            @autoreleasepool {
                if (self.cancelRequested) { finish(@"下载已取消"); return; }
                unsigned long long size = sizes[hash].unsignedLongLongValue;
                totalBytes += size;
                NSString *file = [[objects stringByAppendingPathComponent:[hash substringToIndex:2]]
                                  stringByAppendingPathComponent:hash];
                NSData *cached = [NSData dataWithContentsOfFile:file options:NSDataReadingMappedIfSafe error:nil];
                if (cached && cached.length == size && [NCLRSHA1Hex(cached) isEqualToString:hash]) cachedBytes += size;
                else [pending addObject:hash];
            }
        }

        NSLock *state = [[NSLock alloc] init];
        __block NSUInteger done = sizes.count - pending.count;
        __block unsigned long long transferred = 0;
        NSMutableDictionary<NSString *, NSNumber *> *inFlight = [NSMutableDictionary dictionary];
        __block NSTimeInterval lastUpdate = 0;
        __block NSString *assetError = nil;
        NSTimeInterval started = NSProcessInfo.processInfo.systemUptime;
        unsigned long long expectedBytes = totalBytes;
        unsigned long long bytesOnDisk = cachedBytes;
        void (^reportAssets)(NSTimeInterval) = ^(NSTimeInterval now) {
            unsigned long long currentBytes = transferred;
            for (NSNumber *bytes in inFlight.objectEnumerator) currentBytes += bytes.unsignedLongLongValue;
            float fraction = expectedBytes ? (double)(bytesOnDisk + currentBytes) / expectedBytes : (double)done / sizes.count;
            NSString *speed = [NSByteCountFormatter stringFromByteCount:currentBytes / MAX(0.25, now - started)
                                                             countStyle:NSByteCountFormatterCountStyleFile];
            report([NSString stringWithFormat:@"下载资源 %lu/%lu · %@/s · %d%%",
                    (unsigned long)done, (unsigned long)sizes.count, speed, (int)(100 * fraction)],
                   0.05 + 0.85 * fraction);
        };
        reportAssets(started);
        for (NSString *hash in pending) {
            [assetQueue addOperationWithBlock:^{
                @autoreleasepool {
                    [state lock];
                    BOOL stopped = assetError != nil || self.cancelRequested;
                    [state unlock];
                    if (stopped) return;

                    NSString *prefix = [hash substringToIndex:2];
                    NSString *folder = [objects stringByAppendingPathComponent:prefix];
                    NCLRCreateDirectory(folder);
                    NSString *file = [folder stringByAppendingPathComponent:hash];
                    NSString *url = [NSString stringWithFormat:@"https://resources.download.minecraft.net/%@/%@", prefix, hash];
                    NSString *oneError = nil;
                    NSError *writeError = nil;
                    NSData *data = [self download:url error:&oneError received:^(unsigned long long bytes) {
                        [state lock];
                        inFlight[hash] = @(MIN(bytes, sizes[hash].unsignedLongLongValue));
                        NSTimeInterval now = NSProcessInfo.processInfo.systemUptime;
                        if (!assetError && now - lastUpdate >= 0.12) {
                            lastUpdate = now;
                            reportAssets(now);
                        }
                        [state unlock];
                    }];
                    if (self.cancelRequested) return;
                    if (data == nil || data.length != sizes[hash].unsignedLongLongValue ||
                        ![NCLRSHA1Hex(data) isEqualToString:hash]) {
                        oneError = oneError ?: [NSString stringWithFormat:@"资源校验失败：%@", hash];
                    } else if (![data writeToFile:file options:NSDataWritingAtomic error:&writeError]) {
                        oneError = writeError.localizedDescription;
                    }

                    [state lock];
                    [inFlight removeObjectForKey:hash];
                    if (oneError) {
                        if (!assetError) assetError = oneError;
                    } else if (!assetError) {
                        done++;
                        transferred += data.length;
                        NSTimeInterval now = NSProcessInfo.processInfo.systemUptime;
                        if (now - lastUpdate >= 0.12 || done == sizes.count) {
                            lastUpdate = now;
                            reportAssets(now);
                        }
                    }
                    [state unlock];
                }
            }];
        }
        [assetQueue waitUntilAllOperationsAreFinished];
        if (self.cancelRequested) { finish(@"下载已取消"); return; }
        if (assetError) { finish(assetError); return; }

        dispatch_sync(dispatch_get_main_queue(), ^{ self.cancellable = NO; });
        if (self.cancelRequested) { finish(@"下载已取消"); return; }

        NSString *bundle = [[NSBundle mainBundle] pathForResource:@"ship" ofType:@"hzr3"];
        if (bundle.length == 0) { finish(@"IPA 内缺少 ship.hzr3"); return; }
        report(@"组装游戏资源…", 0.93);
        NSString *destination = out;
        out = [out stringByAppendingString:@".preparing"];
        NSFileManager *files = NSFileManager.defaultManager;
        NSError *cleanupError = nil;
        if ([files fileExistsAtPath:out] && ![files removeItemAtPath:out error:&cleanupError]) {
            finish(cleanupError.localizedDescription);
            return;
        }
        NCLRCreateDirectory([out stringByDeletingLastPathComponent]);
        const char *message = horizon_ios_assemble(bundle.UTF8String, jarPath.UTF8String,
                                                   indexPath.UTF8String, objects.UTF8String,
                                                   out.UTF8String);
        if (message != NULL) {
            finish([NSString stringWithUTF8String:message]);
            return;
        }
        // hzr-device assembles models/data but not the optional 360 MB sound
        // store; copy the official sounds into the finished tree as well.
        report(@"复制原版音效…", 0.96);
        NSString *soundRoot = [out stringByAppendingPathComponent:@"assets/minecraft"];
        for (NSString *name in names) {
            NSString *dest = nil;
            if ([name isEqualToString:@"minecraft/sounds.json"]) {
                dest = [soundRoot stringByAppendingPathComponent:@"sounds.json"];
            } else if ([name hasPrefix:@"minecraft/sounds/"]) {
                dest = [soundRoot stringByAppendingPathComponent:
                        [name substringFromIndex:@"minecraft/".length]];
            } else {
                continue;
            }
            NCLRCreateDirectory([dest stringByDeletingLastPathComponent]);
            if ([[NSFileManager defaultManager] fileExistsAtPath:dest]) continue;
            NSString *hash = objectsMap[name][@"hash"];
            NSString *source = [[objects stringByAppendingPathComponent:[hash substringToIndex:2]]
                                stringByAppendingPathComponent:hash];
            NSError *copyError = nil;
            if (![[NSFileManager defaultManager] copyItemAtPath:source toPath:dest error:&copyError]) {
                finish(copyError.localizedDescription);
                return;
            }
        }
        if (![[NSFileManager defaultManager] fileExistsAtPath:
                [out stringByAppendingPathComponent:@"horizon-resources.json"]]) {
            finish(@"组装完成但资源清单不存在");
            return;
        }
        if (![files fileExistsAtPath:[soundRoot stringByAppendingPathComponent:@"sounds.json"]] ||
            ![files fileExistsAtPath:[soundRoot stringByAppendingPathComponent:@"sounds"]]) {
            finish(@"原版音效资源缺失，无法完成安装");
            return;
        }
        NSError *publishError = nil;
        if ([files fileExistsAtPath:destination] &&
            ![files removeItemAtPath:destination error:&publishError]) {
            finish(publishError.localizedDescription);
            return;
        }
        if (![files moveItemAtPath:out toPath:destination error:&publishError]) {
            finish(publishError.localizedDescription);
            return;
        }
        finish(nil);
    });
}

@end

@interface NCLRRootController : UIViewController <UITableViewDataSource, UITableViewDelegate>
@property (nonatomic, strong) UIView *sidebar;
@property (nonatomic, strong) UIView *content;
@property (nonatomic, strong) UIView *compactBar;
@property (nonatomic, strong) NSArray<UIButton *> *navButtons;
@property (nonatomic, strong) NSArray<UIButton *> *compactButtons;
@property (nonatomic, strong) NSArray *regularConstraints;
@property (nonatomic, strong) NSArray *compactConstraints;
@property (nonatomic, strong) UILabel *accountLabel;
@property (nonatomic, strong) UILabel *accountAvatarLabel;
@property (nonatomic, strong) UIImageView *accountAvatarImage;
@property (nonatomic, assign) NSInteger page;
@property (nonatomic, assign) NSInteger initialPage;
@property (nonatomic, assign) BOOL compactLayout;
@property (nonatomic, strong) UIScrollView *navScroll;
@property (nonatomic, strong) UIStackView *navStack;
@property (nonatomic, strong) NSLayoutConstraint *sidebarWidth;
@property (nonatomic, strong) UITableView *navTable;
@property (nonatomic, copy) NSArray<NSString *> *navNames;
@property (nonatomic, copy) NSArray<NSString *> *navSymbols;
@end

@implementation NCLRRootController
- (void)viewDidLoad {
    [super viewDidLoad];
    self.view.backgroundColor = NCLRCanvas();
    CAGradientLayer *gradient = [CAGradientLayer layer];
    gradient.colors = @[
        (__bridge id)NCLRCanvas().CGColor,
        (__bridge id)NCLRSurface().CGColor,
        (__bridge id)NCLRCanvas().CGColor,
    ];
    gradient.startPoint = CGPointMake(0.1, 0.0);
    gradient.endPoint = CGPointMake(0.9, 1.0);
    [self.view.layer insertSublayer:gradient atIndex:0];

    self.sidebar = [[UIView alloc] init];
    self.sidebar.translatesAutoresizingMaskIntoConstraints = NO;
    self.sidebar.backgroundColor = NCLRSurface();
    self.sidebar.layer.borderWidth = 1.0;
    self.sidebar.layer.borderColor = NCLRBorder().CGColor;
    [self.view addSubview:self.sidebar];

    self.content = [[UIView alloc] init];
    self.content.translatesAutoresizingMaskIntoConstraints = NO;
    [self.view addSubview:self.content];

    // Compact widths (iPhone, iPad split view and resizable windows) get a
    // horizontal navigation bar instead of the 228 pt sidebar.
    self.compactBar = [[UIView alloc] init];
    self.compactBar.translatesAutoresizingMaskIntoConstraints = NO;
    self.compactBar.backgroundColor = NCLRColor(0.02, 0.02, 0.02, 0.45);
    NCLRAddBlur(self.compactBar, NCLRDarkMode);
    self.compactBar.layer.borderWidth = 1.0;
    self.compactBar.layer.borderColor = [UIColor colorWithWhite:1.0 alpha:0.06].CGColor;
    [self.view addSubview:self.compactBar];
    UIStackView *compactNav = [[UIStackView alloc] init];
    compactNav.axis = UILayoutConstraintAxisHorizontal;
    compactNav.distribution = UIStackViewDistributionFillEqually;
    compactNav.spacing = 2.0;
    compactNav.translatesAutoresizingMaskIntoConstraints = NO;
    [self.compactBar addSubview:compactNav];
    NSMutableArray *compact = [NSMutableArray array];
    NSArray *compactNames = @[@"主页", @"版本", @"模组", @"光影", @"资源包", @"设置"];
    for (NSInteger i = 0; i < compactNames.count; i++) {
        UIButton *button = [UIButton buttonWithType:UIButtonTypeSystem];
        button.tag = i;
        button.translatesAutoresizingMaskIntoConstraints = NO;
        button.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.035];
        button.layer.cornerRadius = 10.0;
        button.layer.borderWidth = 1.0;
        button.layer.borderColor = [UIColor colorWithWhite:1.0 alpha:0.05].CGColor;
        button.contentHorizontalAlignment = UIControlContentHorizontalAlignmentLeft;
        button.contentEdgeInsets = UIEdgeInsetsMake(0, 10, 0, 6);
        button.titleLabel.font = [UIFont systemFontOfSize:11.0 weight:UIFontWeightSemibold];
        button.titleLabel.lineBreakMode = NSLineBreakByClipping;
        NSArray *compactSymbols = @[@"house.fill", @"square.stack.3d.up.fill", @"puzzlepiece.extension.fill",
                                    @"sun.max.fill", @"square.grid.2x2.fill",
                                    @"person.crop.circle.fill", @"gearshape.fill"];
        [button setImage:[UIImage systemImageNamed:compactSymbols[i]] forState:UIControlStateNormal];
        button.imageView.contentMode = UIViewContentModeScaleAspectFit;
        button.imageEdgeInsets = UIEdgeInsetsMake(0, 0, 0, 5);
        button.titleEdgeInsets = UIEdgeInsetsMake(0, 5, 0, 0);
        [button setTitle:compactNames[i] forState:UIControlStateNormal];
        [button setTitleColor:NCLRColor(0.92, 0.92, 0.92, 1.0) forState:UIControlStateNormal];
        [button addTarget:self action:@selector(navTapped:) forControlEvents:UIControlEventTouchUpInside];
        [compactNav addArrangedSubview:button];
        [compact addObject:button];
    }
    self.compactButtons = compact;
    [NSLayoutConstraint activateConstraints:@[
        [compactNav.leadingAnchor constraintEqualToAnchor:self.compactBar.leadingAnchor constant:6.0],
        [compactNav.trailingAnchor constraintEqualToAnchor:self.compactBar.trailingAnchor constant:-6.0],
        [compactNav.topAnchor constraintEqualToAnchor:self.compactBar.topAnchor constant:5.0],
        [compactNav.bottomAnchor constraintEqualToAnchor:self.compactBar.bottomAnchor constant:-5.0],
    ]];

    self.sidebarWidth = [self.sidebar.widthAnchor constraintEqualToConstant:228.0];
    self.regularConstraints = @[
        [self.sidebar.leadingAnchor constraintEqualToAnchor:self.view.leadingAnchor],
        [self.sidebar.topAnchor constraintEqualToAnchor:self.view.topAnchor],
        [self.sidebar.bottomAnchor constraintEqualToAnchor:self.view.bottomAnchor],
        self.sidebarWidth,
        [self.content.leadingAnchor constraintEqualToAnchor:self.sidebar.trailingAnchor],
        [self.content.trailingAnchor constraintEqualToAnchor:self.view.trailingAnchor],
        [self.content.topAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.topAnchor],
        [self.content.bottomAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.bottomAnchor],
    ];
    self.compactConstraints = @[
        [self.compactBar.leadingAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.leadingAnchor],
        [self.compactBar.trailingAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.trailingAnchor],
        [self.compactBar.topAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.topAnchor],
        [self.compactBar.heightAnchor constraintEqualToConstant:52.0],
        [self.content.leadingAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.leadingAnchor],
        [self.content.trailingAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.trailingAnchor],
        [self.content.topAnchor constraintEqualToAnchor:self.compactBar.bottomAnchor],
        [self.content.bottomAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.bottomAnchor],
    ];
    [NSLayoutConstraint activateConstraints:self.regularConstraints];
    self.compactBar.hidden = YES;

    UIView *brand = [[UIView alloc] init];
    brand.translatesAutoresizingMaskIntoConstraints = NO;
    UIImage *brandImage = [UIImage imageNamed:@"BrandIcon"];
    UIView *mark = nil;
    if (brandImage != nil) {
        UIImageView *logo = [[UIImageView alloc] initWithImage:brandImage];
        logo.contentMode = UIViewContentModeScaleAspectFit;
        logo.backgroundColor = UIColor.clearColor;
        mark = logo;
    } else {
        UILabel *fallback = [self label:@"N" size:24.0 weight:UIFontWeightHeavy
                                   color:NCLRColor(0.95, 0.95, 0.95, 1.0)];
        fallback.textAlignment = NSTextAlignmentCenter;
        fallback.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.055];
        fallback.layer.cornerRadius = 12.0;
        fallback.layer.masksToBounds = YES;
        mark = fallback;
    }
    mark.layer.cornerRadius = 10.0;
    mark.layer.masksToBounds = YES;
    mark.translatesAutoresizingMaskIntoConstraints = NO;
    UILabel *title = [self label:@"NCLR" size:22.0 weight:UIFontWeightBold
                           color:NCLRColor(1.0, 1.0, 1.0, 1.0)];
    UILabel *subtitle = [self label:@"LAUNCHER" size:8.0 weight:UIFontWeightBold
                             color:NCLRColor(0.50, 0.50, 0.50, 1.0)];
    subtitle.attributedText = [[NSAttributedString alloc] initWithString:@"LAUNCHER" attributes:@{
        NSKernAttributeName : @2.0
    }];
    [brand addSubview:mark];
    [brand addSubview:title];
    [brand addSubview:subtitle];
    [NSLayoutConstraint activateConstraints:@[
        [mark.leadingAnchor constraintEqualToAnchor:brand.leadingAnchor],
        [mark.topAnchor constraintEqualToAnchor:brand.topAnchor],
        [mark.widthAnchor constraintEqualToConstant:34.0],
        [mark.heightAnchor constraintEqualToConstant:34.0],
        [title.leadingAnchor constraintEqualToAnchor:mark.trailingAnchor constant:10.0],
        [title.topAnchor constraintEqualToAnchor:mark.topAnchor constant:1.0],
        [subtitle.leadingAnchor constraintEqualToAnchor:title.leadingAnchor],
        [subtitle.topAnchor constraintEqualToAnchor:title.bottomAnchor constant:2.0],
        [brand.bottomAnchor constraintGreaterThanOrEqualToAnchor:mark.bottomAnchor],
        [brand.bottomAnchor constraintGreaterThanOrEqualToAnchor:subtitle.bottomAnchor],
    ]];
    [self.sidebar addSubview:brand];

    UIScrollView *navScroll = [[UIScrollView alloc] init];
    self.navScroll = navScroll;
    navScroll.translatesAutoresizingMaskIntoConstraints = NO;
    navScroll.backgroundColor = UIColor.clearColor;
    navScroll.showsVerticalScrollIndicator = YES;
    navScroll.alwaysBounceVertical = NO;
    navScroll.clipsToBounds = YES;
    navScroll.contentInset = UIEdgeInsetsMake(0, 0, 8, 0);
    [self.sidebar addSubview:navScroll];

    UIStackView *nav = [[UIStackView alloc] init];
    self.navStack = nav;
    nav.axis = UILayoutConstraintAxisVertical;
    nav.distribution = UIStackViewDistributionFillEqually;
    nav.spacing = 2.0;
    nav.translatesAutoresizingMaskIntoConstraints = NO;
    NSArray *names = @[@"主页", @"版本", @"模组", @"光影", @"资源包", @"设置"];
    NSArray *symbols = @[@"house.fill", @"square.stack.3d.up.fill", @"puzzlepiece.extension.fill", @"sun.max.fill", @"square.grid.2x2.fill", @"gearshape.fill"];
    NSMutableArray *buttons = [NSMutableArray array];
    for (NSInteger i = 0; i < names.count; i++) {
        UIButton *button = [UIButton buttonWithType:UIButtonTypeSystem];
        button.tag = i;
        button.translatesAutoresizingMaskIntoConstraints = NO;
        button.contentHorizontalAlignment = UIControlContentHorizontalAlignmentLeft;
        button.contentEdgeInsets = UIEdgeInsetsMake(0, 14, 0, 8);
        button.titleLabel.font = [UIFont systemFontOfSize:13.0 weight:UIFontWeightSemibold];
        button.titleLabel.lineBreakMode = NSLineBreakByClipping;
        button.layer.cornerRadius = 11.0;
        button.layer.borderWidth = 1.0;
        button.layer.borderColor = [UIColor colorWithWhite:1.0 alpha:0.045].CGColor;
        [button setImage:[UIImage systemImageNamed:symbols[i]] forState:UIControlStateNormal];
        button.imageView.contentMode = UIViewContentModeScaleAspectFit;
        button.imageEdgeInsets = UIEdgeInsetsMake(0, 0, 0, 8);
        button.titleEdgeInsets = UIEdgeInsetsMake(0, 8, 0, 0);
        [button setTitle:names[i] forState:UIControlStateNormal];
        [button setTitleColor:NCLRColor(0.92, 0.92, 0.92, 1.0) forState:UIControlStateNormal];
        [button addTarget:self action:@selector(navTapped:) forControlEvents:UIControlEventTouchUpInside];
        [nav addArrangedSubview:button];
        [buttons addObject:button];
    }
    self.navButtons = buttons;
    [navScroll addSubview:nav];
    navScroll.hidden = NO;

    self.navNames = names;
    self.navSymbols = symbols;
    self.navTable = [[UITableView alloc] initWithFrame:CGRectZero style:UITableViewStylePlain];
    self.navTable.translatesAutoresizingMaskIntoConstraints = NO;
    self.navTable.backgroundColor = UIColor.clearColor;
    self.navTable.separatorStyle = UITableViewCellSeparatorStyleNone;
    self.navTable.showsVerticalScrollIndicator = YES;
    self.navTable.rowHeight = 40.0;
    self.navTable.contentInset = UIEdgeInsetsMake(0, 0, 8, 0);
    self.navTable.dataSource = self;
    self.navTable.delegate = self;
    self.navTable.clipsToBounds = YES;
    self.navTable.hidden = YES;
    [self.sidebar addSubview:self.navTable];

    UIView *account = [[UIView alloc] init];
    account.translatesAutoresizingMaskIntoConstraints = NO;
    account.backgroundColor = NCLRCanvas();
    account.layer.cornerRadius = 12.0;
    UILabel *avatar = [self label:@"N" size:15.0 weight:UIFontWeightBold
                            color:NCLRColor(0.95, 0.95, 0.95, 1.0)];
    self.accountAvatarLabel = avatar;
    avatar.textAlignment = NSTextAlignmentCenter;
    avatar.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.06];
    avatar.layer.cornerRadius = 16.0;
    avatar.layer.masksToBounds = YES;
    avatar.translatesAutoresizingMaskIntoConstraints = NO;
    self.accountLabel = [self label:@"未登录" size:11.0 weight:UIFontWeightSemibold
                              color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    UILabel *accountDetail = [self label:@"Microsoft 账号" size:9.0 weight:UIFontWeightRegular
                                    color:NCLRColor(0.50, 0.50, 0.50, 1.0)];
    UIImageView *avatarImage = [[UIImageView alloc] init];
    avatarImage.translatesAutoresizingMaskIntoConstraints = NO;
    avatarImage.contentMode = UIViewContentModeScaleAspectFill;
    avatarImage.layer.cornerRadius = 16.0;
    avatarImage.layer.masksToBounds = YES;
    avatarImage.hidden = YES;
    self.accountAvatarImage = avatarImage;
    [account addSubview:avatar];
    [account addSubview:avatarImage];
    [account addSubview:self.accountLabel];
    [account addSubview:accountDetail];
    [NSLayoutConstraint activateConstraints:@[
        [avatar.leadingAnchor constraintEqualToAnchor:account.leadingAnchor constant:10.0],
        [avatar.topAnchor constraintEqualToAnchor:account.topAnchor constant:10.0],
        [avatar.widthAnchor constraintEqualToConstant:32.0],
        [avatar.heightAnchor constraintEqualToConstant:32.0],
        [avatarImage.leadingAnchor constraintEqualToAnchor:avatar.leadingAnchor],
        [avatarImage.topAnchor constraintEqualToAnchor:avatar.topAnchor],
        [avatarImage.widthAnchor constraintEqualToAnchor:avatar.widthAnchor],
        [avatarImage.heightAnchor constraintEqualToAnchor:avatar.heightAnchor],
        [self.accountLabel.leadingAnchor constraintEqualToAnchor:avatar.trailingAnchor constant:8.0],
        [self.accountLabel.topAnchor constraintEqualToAnchor:account.topAnchor constant:9.0],
        [self.accountLabel.trailingAnchor constraintEqualToAnchor:account.trailingAnchor constant:-8.0],
        [accountDetail.leadingAnchor constraintEqualToAnchor:self.accountLabel.leadingAnchor],
        [accountDetail.topAnchor constraintEqualToAnchor:self.accountLabel.bottomAnchor constant:2.0],
        [accountDetail.bottomAnchor constraintEqualToAnchor:account.bottomAnchor constant:-9.0],
    ]];
    UITapGestureRecognizer *tap = [[UITapGestureRecognizer alloc] initWithTarget:self action:@selector(accountTapped)];
    [account addGestureRecognizer:tap];
    [self.sidebar addSubview:account];

    [NSLayoutConstraint activateConstraints:@[
        [brand.leadingAnchor constraintEqualToAnchor:self.sidebar.safeAreaLayoutGuide.leadingAnchor constant:18.0],
        [brand.trailingAnchor constraintEqualToAnchor:self.sidebar.safeAreaLayoutGuide.trailingAnchor constant:-18.0],
        [brand.topAnchor constraintEqualToAnchor:self.sidebar.safeAreaLayoutGuide.topAnchor constant:14.0],
        [brand.heightAnchor constraintEqualToConstant:52.0],
        [navScroll.leadingAnchor constraintEqualToAnchor:self.sidebar.safeAreaLayoutGuide.leadingAnchor constant:10.0],
        [navScroll.trailingAnchor constraintEqualToAnchor:self.sidebar.safeAreaLayoutGuide.trailingAnchor constant:-10.0],
        [navScroll.topAnchor constraintEqualToAnchor:brand.bottomAnchor constant:16.0],
        [navScroll.bottomAnchor constraintEqualToAnchor:account.topAnchor constant:-8.0],
        [navScroll.heightAnchor constraintGreaterThanOrEqualToConstant:120.0],
        [nav.leadingAnchor constraintEqualToAnchor:navScroll.contentLayoutGuide.leadingAnchor],
        [nav.trailingAnchor constraintEqualToAnchor:navScroll.contentLayoutGuide.trailingAnchor],
        [nav.topAnchor constraintEqualToAnchor:navScroll.contentLayoutGuide.topAnchor],
        [nav.widthAnchor constraintEqualToAnchor:navScroll.frameLayoutGuide.widthAnchor],
        [nav.heightAnchor constraintEqualToAnchor:navScroll.frameLayoutGuide.heightAnchor],
        [account.leadingAnchor constraintEqualToAnchor:self.sidebar.safeAreaLayoutGuide.leadingAnchor constant:12.0],
        [account.trailingAnchor constraintEqualToAnchor:self.sidebar.safeAreaLayoutGuide.trailingAnchor constant:-12.0],
        [account.bottomAnchor constraintEqualToAnchor:self.sidebar.safeAreaLayoutGuide.bottomAnchor constant:-14.0],
        [account.heightAnchor constraintGreaterThanOrEqualToConstant:56.0],
    ]];

    [[NSNotificationCenter defaultCenter] addObserver:self
                                             selector:@selector(refreshAccount)
                                                 name:NCLRAvatarUpdatedNotification
                                               object:nil];
    [self showPage:self.initialPage];
}

- (UILabel *)label:(NSString *)text size:(CGFloat)size weight:(UIFontWeight)weight color:(UIColor *)color {
    UILabel *label = [[UILabel alloc] init];
    label.text = text;
    label.textColor = color;
    label.font = [UIFont systemFontOfSize:size weight:weight];
    label.numberOfLines = 0;
    label.translatesAutoresizingMaskIntoConstraints = NO;
    return label;
}

- (void)updateAdaptiveLayout {
    CGFloat width = self.view.bounds.size.width;
    BOOL compact = width > 0.0 && width < 720.0;
    NSLayoutConstraint *probe = compact
        ? (NSLayoutConstraint *)self.compactConstraints.firstObject
        : (NSLayoutConstraint *)self.regularConstraints.firstObject;
    if (compact == self.compactLayout && probe.active) {
        return;
    }
    self.compactLayout = compact;
    // Keep the left navigation on every width: the compact top bar made the
    // pages disappear on narrow phones. The sidebar just gets narrower, and
    // its own scroll view holds the page buttons.
    [NSLayoutConstraint deactivateConstraints:self.compactConstraints];
    [NSLayoutConstraint activateConstraints:self.regularConstraints];
    self.sidebarWidth.constant = compact ? 156.0 : 228.0;
    self.sidebar.hidden = NO;
    self.compactBar.hidden = YES;
}

- (void)viewSafeAreaInsetsDidChange {
    [super viewSafeAreaInsetsDidChange];
    [self updateAdaptiveLayout];
}

- (void)viewDidLayoutSubviews {
    [super viewDidLayoutSubviews];
    [self updateAdaptiveLayout];
    if (self.navScroll != nil) {
        CGFloat navHeight = self.navScroll.bounds.size.height;
        self.navScroll.contentSize = CGSizeMake(MAX(1.0, self.navScroll.bounds.size.width), navHeight);
    }
    for (CALayer *layer in self.view.layer.sublayers) {
        if ([layer isKindOfClass:[CAGradientLayer class]]) {
            layer.frame = self.view.bounds;
        }
    }
}

- (void)navTapped:(UIButton *)sender {
    [self showPage:sender.tag];
}

- (void)accountTapped {
    [self showPage:6];
}

- (NSInteger)tableView:(UITableView *)tableView numberOfRowsInSection:(NSInteger)section {
    return self.navNames.count;
}

- (UITableViewCell *)tableView:(UITableView *)tableView cellForRowAtIndexPath:(NSIndexPath *)indexPath {
    static NSString *identifier = @"NCLRNavCell";
    UITableViewCell *cell = [tableView dequeueReusableCellWithIdentifier:identifier];
    if (cell == nil) {
        cell = [[UITableViewCell alloc] initWithStyle:UITableViewCellStyleDefault reuseIdentifier:identifier];
        cell.backgroundColor = UIColor.clearColor;
        cell.textLabel.font = [UIFont systemFontOfSize:13.0 weight:UIFontWeightSemibold];
        cell.textLabel.textColor = NCLRColor(0.95, 0.95, 0.95, 1.0);
        cell.tintColor = NCLRColor(0.95, 0.95, 0.95, 1.0);
        UIView *selected = [[UIView alloc] init];
        selected.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.075];
        selected.layer.cornerRadius = 10.0;
        cell.selectedBackgroundView = selected;
        cell.indentationLevel = 1;
        cell.indentationWidth = 10.0;
    }
    cell.textLabel.text = self.navNames[indexPath.row];
    cell.imageView.image = [UIImage systemImageNamed:self.navSymbols[indexPath.row]];
    return cell;
}

- (void)tableView:(UITableView *)tableView didSelectRowAtIndexPath:(NSIndexPath *)indexPath {
    [self showPage:indexPath.row];
}

- (void)showPage:(NSInteger)page {
    self.page = page;
    if (page >= 0 && page < (NSInteger)self.navNames.count) {
        [self.navTable selectRowAtIndexPath:[NSIndexPath indexPathForRow:page inSection:0]
                                  animated:NO scrollPosition:UITableViewScrollPositionNone];
    }
    for (NSInteger i = 0; i < self.navButtons.count; i++) {
        UIButton *button = self.navButtons[i];
        BOOL selected = i == page;
        button.backgroundColor = selected ? NCLRColor(0.96, 0.96, 0.96, 1.0)
                                          : [UIColor colorWithWhite:1.0 alpha:0.035];
        [button setTitleColor:(selected ? NCLRColor(0.06, 0.06, 0.06, 1.0)
                                        : NCLRColor(0.92, 0.92, 0.92, 1.0))
                     forState:UIControlStateNormal];
        button.tintColor = selected ? NCLRColor(0.06, 0.06, 0.06, 1.0)
                                    : NCLRColor(0.92, 0.92, 0.92, 1.0);
        button.layer.borderColor = selected ? UIColor.clearColor.CGColor
                                            : [UIColor colorWithWhite:1.0 alpha:0.045].CGColor;
        button.alpha = selected ? 1.0 : 0.82;
    }
    for (NSInteger i = 0; i < self.compactButtons.count; i++) {
        UIButton *button = self.compactButtons[i];
        BOOL selected = i == page;
        button.backgroundColor = selected ? [UIColor colorWithWhite:1.0 alpha:0.10] : UIColor.clearColor;
        button.tintColor = selected ? NCLRColor(0.98, 0.98, 0.98, 1.0)
                                    : NCLRColor(0.85, 0.85, 0.85, 1.0);
        button.layer.borderColor = selected ? [UIColor colorWithWhite:1.0 alpha:0.16].CGColor
                                            : [UIColor colorWithWhite:1.0 alpha:0.05].CGColor;
        button.alpha = selected ? 1.0 : 0.72;
    }
    if (self.childViewControllers.count > 0) {
        UIViewController *old = self.childViewControllers.firstObject;
        [old willMoveToParentViewController:nil];
        [old.view removeFromSuperview];
        [old removeFromParentViewController];
    }
    UIViewController *controller = nil;
    switch (page) {
        case 1: controller = [[NCLRVersionsController alloc] init]; break;
        case 2: controller = [[NCLRModsController alloc] init]; break;
        case 3: {
            NCLRPackController *c = [[NCLRPackController alloc] init];
            c.pageTitle = @"光影包";
            c.pageSubtitle = @"导入和管理本地光影包";
            c.directoryName = @"shaderpacks";
            c.emptyText = @"还没有光影包。点击右上角导入 .zip / .hshader，或直接放入 shaderpacks 文件夹。";
            c.extensions = @[@"zip", @"hshader"];
            c.noteText = @"目录：Documents/shaderpacks。经典 shaders/ 文件夹、.zip 或 .hshader 都可被 Horizon 读取。";
            controller = c;
            break;
        }
        case 4: {
            NCLRPackController *c = [[NCLRPackController alloc] init];
            c.pageTitle = @"资源包";
            c.pageSubtitle = @"导入和管理本地资源包";
            c.directoryName = @"resourcepacks";
            c.emptyText = @"还没有资源包。点击右上角导入 .zip，或直接放入 resourcepacks 文件夹。";
            c.extensions = @[@"zip"];
            c.noteText = @"目录：Documents/resourcepacks。导入后在游戏内资源包页面启用。";
            controller = c;
            break;
        }
        case 5: controller = [[NCLRSettingsController alloc] init]; break;
        case 6: controller = [[NCLRAccountController alloc] init]; break;
        default: controller = [[NCLRHomeController alloc] init]; break;
    }
    if ([controller isKindOfClass:[NCLRPageController class]]) {
        __weak typeof(self) weakSelf = self;
        ((NCLRPageController *)controller).onDataChanged = ^{ [weakSelf showPage:weakSelf.page]; };
    }
    [self addChildViewController:controller];
    controller.view.translatesAutoresizingMaskIntoConstraints = NO;
    [self.content addSubview:controller.view];
    [NSLayoutConstraint activateConstraints:@[
        [controller.view.leadingAnchor constraintEqualToAnchor:self.content.leadingAnchor],
        [controller.view.trailingAnchor constraintEqualToAnchor:self.content.trailingAnchor],
        [controller.view.topAnchor constraintEqualToAnchor:self.content.topAnchor],
        [controller.view.bottomAnchor constraintEqualToAnchor:self.content.bottomAnchor],
    ]];
    [controller didMoveToParentViewController:self];
    if (!UIAccessibilityIsReduceMotionEnabled()) {
        controller.view.alpha = 0.0;
        controller.view.transform = CGAffineTransformMakeTranslation(0, 10.0);
        [UIView animateWithDuration:0.24 delay:0.0
                            options:UIViewAnimationOptionCurveEaseOut | UIViewAnimationOptionAllowUserInteraction
                         animations:^{
            controller.view.alpha = 1.0;
            controller.view.transform = CGAffineTransformIdentity;
        } completion:nil];
    }
    [self refreshAccount];
}

- (void)refreshAccount {
    NCLRAuth *auth = NCLRAuth.shared;
    BOOL premium = auth.token.length > 0;
    self.accountLabel.text = premium ? (auth.name ?: @"正版用户") : @"未登录";
    NCLRLoadRemoteAvatar(auth.avatarURL, self.accountAvatarImage, self.accountAvatarLabel);
}
@end

#pragma mark - App

@interface NCLRSceneDelegate : UIResponder <UIWindowSceneDelegate>
@property (nonatomic, strong) UIWindow *window;
@end

@implementation NCLRSceneDelegate
- (void)scene:(UIScene *)scene
    willConnectToSession:(UISceneSession *)session
                 options:(UISceneConnectionOptions *)connectionOptions {
    if (![scene isKindOfClass:[UIWindowScene class]]) {
        return;
    }
    UIWindowScene *windowScene = (UIWindowScene *)scene;
    NCLRThemeLoad();
    self.window = [[UIWindow alloc] initWithWindowScene:windowScene];
    self.window.rootViewController = [[NCLRRootController alloc] init];
    self.window.windowLevel = UIWindowLevelNormal;
    self.window.overrideUserInterfaceStyle = NCLRDarkMode ? UIUserInterfaceStyleDark : UIUserInterfaceStyleLight;
    [self.window makeKeyAndVisible];
}
@end

@interface NCLRAppDelegate : UIResponder <UIApplicationDelegate>
@end

@implementation NCLRAppDelegate
- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {
    return YES;
}
@end

int main(int argc, char *argv[]) {
    @autoreleasepool {
        // The launcher owns UIApplicationMain. The engine is started later,
        // from launchTapped, through patched winit's late-start path.
        return UIApplicationMain(argc, argv, nil, NSStringFromClass([NCLRAppDelegate class]));
    }
}

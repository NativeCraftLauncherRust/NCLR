#import <UIKit/UIKit.h>
#import <Security/Security.h>
#import <stdlib.h>

extern int horizon_ios_run(const char *resources, const char *game_dir);


static NSString *const kNCLRService = @"dev.zrxmaxlnx.nclr.launcher";
static NSString *const kNCLRAccount = @"minecraft-access-token";
static NSString *const kNCLRClientKey = @"NCLRMicrosoftClientID";
static NSString *const kNCLRAccountNameKey = @"NCLRAccountName";

static UIColor *NCLRColor(CGFloat r, CGFloat g, CGFloat b, CGFloat a) {
    return [UIColor colorWithRed:r green:g blue:b alpha:a];
}

static NSString *NCLRString(id value) {
    return [value isKindOfClass:[NSString class]] ? value : nil;
}

static NSString *NCLRDocuments(void) {
    NSArray<NSString *> *paths =
        NSSearchPathForDirectoriesInDomains(NSDocumentDirectory, NSUserDomainMask, YES);
    return paths.firstObject ?: NSTemporaryDirectory();
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


static void NCLRBoostLayerTree(CALayer *layer, NSMutableString *log, int depth);

@interface NCLRFramePump : NSObject
@end

@implementation NCLRFramePump {
    NSInteger _ticks;
}

- (void)fire:(CADisplayLink *)link {
    _ticks++;
    if (_ticks % 30 != 0) return;
    NSMutableString *log = [NSMutableString string];
    NSArray *windows = nil;
    @try { windows = [UIApplication.sharedApplication.windows copy]; } @catch (NSException *e) { (void)e; }
    for (UIWindow *window in windows) NCLRBoostLayerTree(window.layer, log, 0);
    [log appendFormat:@"displayLink preferred=%ld duration=%.6f timestamp=%.3f screenMax=%ld lowPower=%d\n",
        (long)link.preferredFramesPerSecond, link.duration, link.timestamp,
        (long)UIScreen.mainScreen.maximumFramesPerSecond,
        (int)NSProcessInfo.processInfo.isLowPowerModeEnabled];
    NSString *path = [NCLRDocuments() stringByAppendingPathComponent:@"nclr-layer.txt"];
    [log writeToFile:path atomically:YES encoding:NSUTF8StringEncoding error:nil];
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

@interface NCLRKeymap : NSObject
@property (nonatomic, strong) NSMutableDictionary<NSString *, NSString *> *bindings;
@property (nonatomic, strong) NSMutableDictionary<NSString *, NSDictionary *> *positions;
@property (nonatomic, assign) CGFloat buttonSize;
@property (nonatomic, assign) CGFloat opacity;
@property (nonatomic, copy) NSString *touchMode;
@property (nonatomic, assign) BOOL dynamicJoystick;
@property (nonatomic, assign) NSInteger layoutVersion;
+ (instancetype)shared;
- (void)load;
- (void)save;
- (NSString *)path;
- (void)addKey:(NSString *)key label:(NSString *)label;
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
    } mutableCopy];
    self.opacity = 0.55;
    self.layoutVersion = 2;
    self.touchMode = @"crosshair";
    self.dynamicJoystick = NO;
    self.positions = [@{
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
            NSDictionary *value = layout[key];
            if ([value isKindOfClass:[NSDictionary class]]) {
                self.positions[key] = value;
            }
        }
    }
    NSInteger savedVersion = [json[@"layoutVersion"] integerValue];
    if (savedVersion < 2) {
        [self resetLayout];
        [self save];
    }
    if (self.bindings[@"move"] == nil) {
        self.bindings[@"move"] = @"移动";
    }
    if (self.positions[@"move"] == nil) {
        self.positions[@"move"] = @{ @"x" : @0.16, @"y" : @0.78, @"size" : @0.30 };
    }
}

- (void)addKey:(NSString *)key label:(NSString *)label {
    if (key.length == 0 || self.bindings[key] != nil) {
        return;
    }
    self.bindings[key] = label.length > 0 ? label : key;
    self.positions[key] = @{ @"x" : @0.50, @"y" : @0.50, @"size" : @0.12 };
}

- (void)resetLayout {
    self.positions = [@{
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
        @"move" : @{ @"x" : @0.14, @"y" : @0.74, @"size" : @0.28 },
    } mutableCopy];
}

- (void)save {
    NSDictionary *json = @{
        @"layoutVersion" : @(self.layoutVersion),
        @"buttonSize" : @(self.buttonSize),
        @"opacity" : @(self.opacity),
        @"touchMode" : self.touchMode ?: @"crosshair",
        @"dynamicJoystick" : @(self.dynamicJoystick),
        @"bindings" : self.bindings ?: @{},
        @"layout" : self.positions ?: @{},
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
    self.clientId = [[NSUserDefaults standardUserDefaults] stringForKey:kNCLRClientKey] ?: @"";
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

typedef void (^NCLRAuthCompletion)(NSString *token, NSString *name, NSString *error);

@interface NCLRAuth : NSObject
+ (instancetype)shared;
- (void)startWithClientId:(NSString *)clientId
                presenter:(UIViewController *)presenter
               completion:(NCLRAuthCompletion)completion;
- (void)logout;
@property (nonatomic, copy) NSString *token;
@property (nonatomic, copy) NSString *name;
@property (nonatomic, assign) BOOL developerPremium;
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
        // L1 developer build: treat the player as a premium owner until a real
        // account is signed in. The game launch is not gated by this flag.
        _developerPremium = YES;
    }
    return self;
}

- (void)logout {
    self.token = nil;
    self.name = nil;
    NCLRKeychainWrite(nil);
    [[NSUserDefaults standardUserDefaults] removeObjectForKey:kNCLRAccountNameKey];
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
            [alert addAction:[UIAlertAction actionWithTitle:@"打开浏览器" style:UIAlertActionStyleDefault
                handler:^(UIAlertAction *action) {
                    NSURL *url = verification.length > 0 ? [NSURL URLWithString:verification] : nil;
                    if (url) {
                        [UIApplication.sharedApplication openURL:url options:@{} completionHandler:nil];
                    }
                }]];
            [alert addAction:[UIAlertAction actionWithTitle:@"开始等待" style:UIAlertActionStyleDefault
                handler:^(UIAlertAction *action) {
                    [weakSelf pollWithClientId:clientId
                                    deviceCode:deviceCode
                                      interval:MAX(interval, 5)
                                     presenter:presenter
                                    completion:completion];
                }]];
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

- (void)fetchProfileWithToken:(NSString *)token completion:(NCLRAuthCompletion)completion {
    NSMutableURLRequest *request = [NSMutableURLRequest
        requestWithURL:[NSURL URLWithString:@"https://api.minecraftservices.com/minecraft/profile"]];
    [request setValue:[NSString stringWithFormat:@"Bearer %@", token]
   forHTTPHeaderField:@"Authorization"];
    [[NSURLSession.sharedSession dataTaskWithRequest:request
        completionHandler:^(NSData *data, NSURLResponse *response, NSError *error) {
            NSString *name = @"Minecraft 玩家";
            if (data.length > 0) {
                NSDictionary *json = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
                if ([json isKindOfClass:[NSDictionary class]]) {
                    name = NCLRString(json[@"name"]) ?: name;
                }
            }
            NCLRAuth *auth = NCLRAuth.shared;
            auth.token = token;
            auth.name = name;
            NCLRKeychainWrite(token);
            [[NSUserDefaults standardUserDefaults] setObject:name forKey:kNCLRAccountNameKey];
            dispatch_async(dispatch_get_main_queue(), ^{ completion(token, name, nil); });
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
    card.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.052];
    card.layer.cornerRadius = 18.0;
    card.layer.borderWidth = 1.0;
    card.layer.borderColor = [UIColor colorWithWhite:1.0 alpha:0.075].CGColor;
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
        [button setTitleColor:NCLRColor(0.04, 0.04, 0.04, 1.0) forState:UIControlStateNormal];
        button.backgroundColor = NCLRColor(0.95, 0.95, 0.95, 1.0);
    } else {
        [button setTitleColor:NCLRColor(0.94, 0.94, 0.94, 1.0) forState:UIControlStateNormal];
        button.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.075];
        button.layer.borderWidth = 1.0;
        button.layer.borderColor = [UIColor colorWithWhite:1.0 alpha:0.12].CGColor;
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

@interface NCLRHomeController : NCLRPageController
@end

@implementation NCLRHomeController
- (void)viewDidLoad {
    [super viewDidLoad];
    [self setupPage];
    NCLRStore *store = NCLRStore.shared;
    NCLRVersion *selected = store.selected;
    [self.stack addArrangedSubview:[self headerWithTitle:@"主页"
                                                subtitle:@"Minecraft: Java Edition · NCLR"
                                             actionTitle:nil action:nil]];

    UIView *hero = [self card];
    hero.backgroundColor = NCLRColor(0.055, 0.055, 0.055, 1.0);
    hero.layer.borderColor = NCLRColor(1.0, 1.0, 1.0, 0.20).CGColor;
    UILabel *heroEyebrow = [self eyebrow:@"SELECTED PROFILE"];
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
    UILabel *hint = [self label:@"启动器与游戏端分离；版本、模组和账号由 NCLR 管理。"
                           size:10.0 weight:UIFontWeightRegular
                          color:NCLRColor(0.50, 0.50, 0.50, 1.0)];
    UIStackView *heroStack = [[UIStackView alloc] initWithArrangedSubviews:
        @[heroEyebrow, heroTitle, heroDetail, launch, hint]];
    heroStack.axis = UILayoutConstraintAxisVertical;
    heroStack.spacing = 8.0;
    heroStack.translatesAutoresizingMaskIntoConstraints = NO;
    [hero addSubview:heroStack];
    [NSLayoutConstraint activateConstraints:@[
        [heroStack.leadingAnchor constraintEqualToAnchor:hero.leadingAnchor constant:20.0],
        [heroStack.trailingAnchor constraintEqualToAnchor:hero.trailingAnchor constant:-20.0],
        [heroStack.topAnchor constraintEqualToAnchor:hero.topAnchor constant:18.0],
        [heroStack.bottomAnchor constraintEqualToAnchor:hero.bottomAnchor constant:-18.0],
    ]];
    [self.stack addArrangedSubview:hero];

    UIStackView *stats = [[UIStackView alloc] init];
    stats.axis = UILayoutConstraintAxisHorizontal;
    stats.spacing = 10.0;
    stats.distribution = UIStackViewDistributionFillEqually;
    NSArray *statTitles = @[@"版本", @"模组", @"账号"];
    BOOL premium = NCLRAuth.shared.developerPremium || NCLRAuth.shared.token.length > 0;
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

    [self.stack addArrangedSubview:[self eyebrow:@"LIBRARY"]];
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

    UIView *activity = [self card];
    UILabel *activityTitle = [self eyebrow:@"状态"];
    UILabel *activityText = [self label:@"启动器已就绪。版本切换和模组列表保存在本机；游戏端通过 nclr-game:// 接入。"
                                    size:12.0 weight:UIFontWeightRegular
                                   color:NCLRColor(0.70, 0.70, 0.70, 1.0)];
    UIStackView *activityStack = [[UIStackView alloc] initWithArrangedSubviews:@[activityTitle, activityText]];
    activityStack.axis = UILayoutConstraintAxisVertical;
    activityStack.spacing = 6.0;
    activityStack.translatesAutoresizingMaskIntoConstraints = NO;
    [activity addSubview:activityStack];
    [NSLayoutConstraint activateConstraints:@[
        [activityStack.leadingAnchor constraintEqualToAnchor:activity.leadingAnchor constant:16.0],
        [activityStack.trailingAnchor constraintEqualToAnchor:activity.trailingAnchor constant:-16.0],
        [activityStack.topAnchor constraintEqualToAnchor:activity.topAnchor constant:14.0],
        [activityStack.bottomAnchor constraintEqualToAnchor:activity.bottomAnchor constant:-14.0],
    ]];
    [self.stack addArrangedSubview:activity];
}

- (void)writePerformanceOptions {
    NSString *path = [NCLRDocuments() stringByAppendingPathComponent:@"options.txt"];
    NSMutableArray<NSString *> *lines = [[[NSString stringWithContentsOfFile:path
                                                                  encoding:NSUTF8StringEncoding
                                                                     error:nil]
        componentsSeparatedByString:@"\n"] mutableCopy] ?: [NSMutableArray array];
    NSDictionary<NSString *, NSString *> *wanted = @{
        @"maxFps" : @"260",
        @"batteryMode" : @"false",
        @"enableVsync" : @"false",
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

static void NCLRBoostLayerTree(CALayer *layer, NSMutableString *log, int depth) {
    if (layer == nil || depth > 12) return;
    if ([layer isKindOfClass:NSClassFromString(@"CAMetalLayer")]) {
        id oldSync = nil;
        @try { oldSync = [layer valueForKey:@"displaySyncEnabled"]; } @catch (NSException *e) { (void)e; }
        @try { [layer setValue:@NO forKey:@"displaySyncEnabled"]; } @catch (NSException *e) { (void)e; }
        @try { [layer setValue:@NO forKey:@"presentsWithTransaction"]; } @catch (NSException *e) { (void)e; }
        @try { [layer setValue:@120 forKey:@"maximumFramesPerSecond"]; } @catch (NSException *e) { (void)e; }
        [log appendFormat:@"layer=%@ oldSync=%@ screenMax=%ld lowPower=%d\n",
            NSStringFromClass(layer.class), oldSync,
            (long)UIScreen.mainScreen.maximumFramesPerSecond,
            (int)NSProcessInfo.processInfo.isLowPowerModeEnabled];
    }
    NSArray *sublayers = nil;
    @try { sublayers = [layer.sublayers copy]; } @catch (NSException *e) { (void)e; }
    for (CALayer *sub in sublayers) NCLRBoostLayerTree(sub, log, depth + 1);
}

- (void)boostMetalLayers:(UIView *)view {
    if (view == nil) return;
    if ([view.layer isKindOfClass:NSClassFromString(@"CAMetalLayer")]) {
        id oldSync = nil;
        @try { oldSync = [view.layer valueForKey:@"displaySyncEnabled"]; } @catch (NSException *e) { (void)e; }
        @try { [view.layer setValue:@NO forKey:@"displaySyncEnabled"]; } @catch (NSException *e) { (void)e; }
        @try { [view.layer setValue:@NO forKey:@"presentsWithTransaction"]; } @catch (NSException *e) { (void)e; }
        NSString *line = [NSString stringWithFormat:@"view=%@ layer=%@ oldSync=%@ screenMax=%ld\n",
                          NSStringFromClass(view.class), NSStringFromClass(view.layer.class),
                          oldSync, (long)UIScreen.mainScreen.maximumFramesPerSecond];
        NSString *path = [NCLRDocuments() stringByAppendingPathComponent:@"nclr-layer.txt"];
        FILE *f = fopen(path.UTF8String, "a");
        if (f) { fputs(line.UTF8String, f); fclose(f); }
    }
    for (UIView *sub in view.subviews) [self boostMetalLayers:sub];
}

- (void)launchTapped {
    [self writePerformanceOptions];
    NSString *resources = [[[NSBundle mainBundle] resourcePath]
        stringByAppendingPathComponent:@"horizon-client/26.3"];
    if (![[NSFileManager defaultManager] fileExistsAtPath:
            [resources stringByAppendingPathComponent:@"horizon-resources.json"]]) {
        UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"资源缺失"
                                                                       message:@"horizon-client/26.3 不存在或不完整。"
                                                                preferredStyle:UIAlertControllerStyleAlert];
        [alert addAction:[UIAlertAction actionWithTitle:@"好" style:UIAlertActionStyleDefault handler:nil]];
        [self presentViewController:alert animated:YES completion:nil];
        return;
    }
    NSString *game = NCLRDocuments();
    NCLRCreateDirectory(game);
    setenv("HORIZON_HOME", game.UTF8String, 1);
    setenv("HORIZON_NO_AUDIO", "1", 1);
    setenv("HORIZON_NO_DISCORD", "1", 1);
    // This call starts winit only now. The patched winit attaches to the
    // already-running UIApplication and drives its loop without a second
    // UIApplicationMain.
    id sceneDelegate = self.view.window.windowScene.delegate;
    UIWindow *launcherWindow = self.view.window;
    launcherWindow.userInteractionEnabled = NO;
    [launcherWindow resignKeyWindow];
    launcherWindow.hidden = YES;
    NSThread *boostThread = [[NSThread alloc] initWithBlock:^{
        @autoreleasepool {
            NCLRFramePump *pump = [[NCLRFramePump alloc] init];
            CADisplayLink *link = [CADisplayLink displayLinkWithTarget:pump selector:@selector(fire:)];
            link.preferredFramesPerSecond = 120;
            [link addToRunLoop:[NSRunLoop currentRunLoop] forMode:NSRunLoopCommonModes];
            [[NSRunLoop currentRunLoop] run];
        }
    }];
    [boostThread start];
    horizon_ios_run(resources.UTF8String, game.UTF8String);
}
@end

@interface NCLRVersionsController : NCLRPageController
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
    UILabel *note = [self label:@"版本配置会保存在本机。实际下载、校验和游戏启动由游戏端完成；启动器不绑定引擎生命周期。"
                            size:10.0 weight:UIFontWeightRegular
                           color:NCLRColor(0.50, 0.50, 0.50, 1.0)];
    [self.stack addArrangedSubview:note];
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
    BOOL premium = auth.developerPremium || auth.token.length > 0;
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
    NSString *nameText = premium ? (auth.name ?: @"正版用户（开发者默认）") : @"未登录";
    UILabel *name = [self label:nameText
                            size:20.0 weight:UIFontWeightSemibold
                           color:NCLRColor(1.0, 1.0, 1.0, 1.0)];
    NSString *detailText = auth.token.length > 0
        ? @"Minecraft Services token 已保存在 Keychain"
        : @"L1 开发者测试版默认正版，可直接启动游戏";
    UILabel *detail = [self label:detailText
                             size:11.0 weight:UIFontWeightRegular
                            color:NCLRColor(0.58, 0.58, 0.58, 1.0)];
    NSString *loginText = auth.token.length > 0 ? @"退出登录" : @"登录正式账号";
    UIButton *login = [self button:loginText
                            action:@selector(loginTapped) primary:auth.token.length == 0];
    [account addSubview:avatar];
    [account addSubview:name];
    [account addSubview:detail];
    [account addSubview:login];
    [NSLayoutConstraint activateConstraints:@[
        [avatar.leadingAnchor constraintEqualToAnchor:account.leadingAnchor constant:16.0],
        [avatar.topAnchor constraintEqualToAnchor:account.topAnchor constant:16.0],
        [avatar.widthAnchor constraintEqualToConstant:46.0],
        [avatar.heightAnchor constraintEqualToConstant:46.0],
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

    UIView *client = [self card];
    UILabel *clientTitle = [self eyebrow:@"AZURE CLIENT ID"];
    UILabel *clientValue = [self label:NCLRStore.shared.clientId.length > 0
                                        ? NCLRStore.shared.clientId : @"未配置"
                                 size:12.0 weight:UIFontWeightRegular
                                color:NCLRColor(0.70, 0.70, 0.70, 1.0)];
    UIButton *configure = [self button:@"配置" action:@selector(configClientId) primary:NO];
    [client addSubview:clientTitle];
    [client addSubview:clientValue];
    [client addSubview:configure];
    [NSLayoutConstraint activateConstraints:@[
        [clientTitle.leadingAnchor constraintEqualToAnchor:client.leadingAnchor constant:16.0],
        [clientTitle.topAnchor constraintEqualToAnchor:client.topAnchor constant:14.0],
        [clientValue.leadingAnchor constraintEqualToAnchor:clientTitle.leadingAnchor],
        [clientValue.topAnchor constraintEqualToAnchor:clientTitle.bottomAnchor constant:5.0],
        [clientValue.trailingAnchor constraintLessThanOrEqualToAnchor:configure.leadingAnchor constant:-10.0],
        [clientValue.bottomAnchor constraintEqualToAnchor:client.bottomAnchor constant:-14.0],
        [configure.trailingAnchor constraintEqualToAnchor:client.trailingAnchor constant:-14.0],
        [configure.centerYAnchor constraintEqualToAnchor:client.centerYAnchor],
        [configure.widthAnchor constraintEqualToConstant:72.0],
    ]];
    [self.stack addArrangedSubview:client];

    UILabel *legal = [self label:@"启动器只在你主动登录时联系 Microsoft / Minecraft Services。不会内置公共 Client ID，也不提供离线绕过。"
                             size:10.0 weight:UIFontWeightRegular
                            color:NCLRColor(0.50, 0.50, 0.50, 1.0)];
    [self.stack addArrangedSubview:legal];
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
        [self configClientId];
        return;
    }
    __weak typeof(self) weakSelf = self;
    [NCLRAuth.shared startWithClientId:NCLRStore.shared.clientId
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
        @[@"Space", @"F1", @"F3", @"F5", @"F"],
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

@interface NCLRKeymapController : NCLRPageController
@property (nonatomic, strong) UISlider *sizeSlider;
@property (nonatomic, strong) UILabel *sizeValue;
@property (nonatomic, strong) UIButton *menuToggle;
@property (nonatomic, strong) UIStackView *sizeRow;
@property (nonatomic, strong) UISlider *opacitySlider;
@property (nonatomic, strong) UILabel *opacityValue;
@property (nonatomic, strong) UISegmentedControl *modeControl;
@property (nonatomic, strong) UISwitch *dynamicSwitch;
@property (nonatomic, strong) NSMutableArray<UIButton *> *keyButtons;
@property (nonatomic, strong) UIView *preview;
@property (nonatomic, strong) UIView *grid;
@property (nonatomic, strong) UIView *selection;
@property (nonatomic, strong) UIView *resizeHandle;
@property (nonatomic, strong) UILabel *selectionInfo;
@property (nonatomic, strong) NSMutableDictionary<NSString *, UIButton *> *previewButtons;
@property (nonatomic, copy) NSString *selectedKey;
@property (nonatomic, assign) CGPoint dragStartCenter;
@property (nonatomic, strong) NSLayoutConstraint *previewHeight;
@property (nonatomic, strong) UIView *previewCard;
@property (nonatomic, strong) UIButton *canvasClose;
@property (nonatomic, assign) BOOL canvasFullscreen;
@end

@interface NCLRSettingsController : NCLRPageController
@end

@implementation NCLRSettingsController
- (void)viewDidLoad {
    [super viewDidLoad];
    [self setupPage];
    [self.stack addArrangedSubview:[self headerWithTitle:@"设置" subtitle:@"启动器、存储和关于"
                                             actionTitle:nil action:nil]];

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
    UILabel *aboutText = [self label:@"NCLR Launcher L1 · 开发者测试版\n作者 ZRXMAXLNX、可心\nMinecraft 是 Mojang / Microsoft 的产品；本项目不是官方产品。"
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

@interface NCLRGridOverlay : UIView
@end
@implementation NCLRGridOverlay
- (void)drawRect:(CGRect)rect {
    CGContextRef ctx = UIGraphicsGetCurrentContext();
    CGContextSetStrokeColorWithColor(ctx, [UIColor colorWithWhite:1.0 alpha:0.10].CGColor);
    CGContextSetLineWidth(ctx, 0.5);
    for (NSInteger i = 1; i < 10; i++) {
        CGFloat x = CGRectGetWidth(rect) * i / 10.0;
        CGFloat y = CGRectGetHeight(rect) * i / 10.0;
        CGContextMoveToPoint(ctx, x, 0); CGContextAddLineToPoint(ctx, x, CGRectGetHeight(rect));
        CGContextMoveToPoint(ctx, 0, y); CGContextAddLineToPoint(ctx, CGRectGetWidth(rect), y);
    }
    CGContextStrokePath(ctx);
    CGContextSetStrokeColorWithColor(ctx, [UIColor colorWithWhite:1.0 alpha:0.28].CGColor);
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
    self.previewButtons = [NSMutableDictionary dictionary];
    [self setupPage];
    [self.stack addArrangedSubview:[self headerWithTitle:@"触控界面"
                                                subtitle:@"游戏内触控按钮 · 布局、位置与大小"
                                             actionTitle:nil action:nil]];

    UIView *menu = [self card];
    self.menuToggle = [self button:@"触控设置  ▾" action:@selector(toggleMenu) primary:NO];
    self.menuToggle.contentHorizontalAlignment = UIControlContentHorizontalAlignmentLeft;
    self.menuToggle.titleLabel.font = [UIFont systemFontOfSize:14.0 weight:UIFontWeightSemibold];
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
    self.sizeRow.hidden = YES;
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
    UIButton *addKey = [self button:@"+ 添加触控键" action:@selector(addKeyTapped) primary:NO];
    UIButton *gridButton = [self button:@"网格" action:@selector(toggleGrid) primary:NO];
    UIButton *resetButton = [self button:@"重置布局" action:@selector(resetLayoutTapped) primary:NO];
    UIButton *fullButton = [self button:@"全屏编辑" action:@selector(toggleCanvas) primary:NO];
    UIStackView *tools = [[UIStackView alloc] initWithArrangedSubviews:@[gridButton, resetButton, fullButton]];
    tools.axis = UILayoutConstraintAxisHorizontal;
    tools.spacing = 8.0;
    tools.distribution = UIStackViewDistributionFillEqually;
    UIStackView *menuStack = [[UIStackView alloc] initWithArrangedSubviews:
        @[self.menuToggle, self.sizeRow, opacityRow, self.modeControl, dynamicRow, addKey, tools]];
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
    [self.stack addArrangedSubview:menu];

    UIView *previewCard = [self card];
    UILabel *previewLabel = [self eyebrow:@"触控布局预览 · 拖动按钮调整位置"];
    self.preview = [[UIView alloc] init];
    self.preview.translatesAutoresizingMaskIntoConstraints = NO;
    self.preview.backgroundColor = NCLRColor(0.02, 0.02, 0.02, 1.0);
    self.preview.layer.cornerRadius = 12.0;
    self.preview.layer.borderWidth = 1.0;
    self.preview.layer.borderColor = [UIColor colorWithWhite:1.0 alpha:0.08].CGColor;
    self.preview.clipsToBounds = YES;
    [previewCard addSubview:previewLabel];
    [previewCard addSubview:self.preview];
    self.selectionInfo = [self label:@"选择一个按钮后拖动位置，拖右下角圆点调整大小"
                                size:10.0 weight:UIFontWeightRegular
                               color:NCLRColor(0.62, 0.62, 0.62, 1.0)];
    [previewCard addSubview:self.selectionInfo];
    self.previewCard = previewCard;
    self.previewHeight = [self.preview.heightAnchor constraintEqualToConstant:200.0];
    self.canvasClose = [self button:@"退出全屏" action:@selector(toggleCanvas) primary:NO];
    self.canvasClose.hidden = YES;
    [self.view addSubview:self.canvasClose];
    [NSLayoutConstraint activateConstraints:@[
        [self.canvasClose.trailingAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.trailingAnchor constant:-16.0],
        [self.canvasClose.topAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.topAnchor constant:12.0],
        [self.canvasClose.widthAnchor constraintEqualToConstant:104.0],
    ]];
    [NSLayoutConstraint activateConstraints:@[
        [previewLabel.leadingAnchor constraintEqualToAnchor:previewCard.leadingAnchor constant:16.0],
        [previewLabel.topAnchor constraintEqualToAnchor:previewCard.topAnchor constant:12.0],
        [self.preview.leadingAnchor constraintEqualToAnchor:previewCard.leadingAnchor constant:12.0],
        [self.preview.trailingAnchor constraintEqualToAnchor:previewCard.trailingAnchor constant:-12.0],
        [self.preview.topAnchor constraintEqualToAnchor:previewLabel.bottomAnchor constant:8.0],
        self.previewHeight,
        [self.selectionInfo.leadingAnchor constraintEqualToAnchor:self.preview.leadingAnchor],
        [self.selectionInfo.trailingAnchor constraintEqualToAnchor:self.preview.trailingAnchor],
        [self.selectionInfo.topAnchor constraintEqualToAnchor:self.preview.bottomAnchor constant:6.0],
        [self.selectionInfo.bottomAnchor constraintEqualToAnchor:previewCard.bottomAnchor constant:-10.0],
    ]];
    self.grid = [[NCLRGridOverlay alloc] initWithFrame:self.preview.bounds];
    self.grid.autoresizingMask = UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
    self.grid.userInteractionEnabled = NO;
    self.grid.backgroundColor = UIColor.clearColor;
    [self.preview addSubview:self.grid];

    self.selection = [[UIView alloc] initWithFrame:CGRectZero];
    self.selection.userInteractionEnabled = NO;
    self.selection.backgroundColor = UIColor.clearColor;
    self.selection.layer.borderWidth = 2.0;
    self.selection.layer.borderColor = NCLRColor(1.0, 1.0, 1.0, 0.95).CGColor;
    self.selection.layer.cornerRadius = 10.0;
    self.selection.hidden = YES;
    [self.preview addSubview:self.selection];

    self.resizeHandle = [[UIView alloc] initWithFrame:CGRectMake(0, 0, 24, 24)];
    self.resizeHandle.backgroundColor = UIColor.whiteColor;
    self.resizeHandle.layer.cornerRadius = 12.0;
    self.resizeHandle.layer.borderWidth = 2.0;
    self.resizeHandle.layer.borderColor = NCLRColor(0.05, 0.05, 0.05, 0.9).CGColor;
    self.resizeHandle.hidden = YES;
    self.resizeHandle.userInteractionEnabled = YES;
    [self.resizeHandle addGestureRecognizer:[[UIPanGestureRecognizer alloc] initWithTarget:self
                                                                                   action:@selector(resizeKey:)]];
    [self.preview addSubview:self.resizeHandle];

    for (NSString *key in NCLRKeymap.shared.bindings) {
        if ([key isEqualToString:@"forward"] || [key isEqualToString:@"back"] ||
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
            knob.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.30];
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
            [button setTitle:NCLRKeymap.shared.bindings[key] forState:UIControlStateNormal];
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
    [self.stack addArrangedSubview:previewCard];

    NSArray<NSString *> *names = @[@"移动摇杆", @"跳跃", @"潜行", @"冲刺", @"攻击",
                                   @"使用", @"背包", @"聊天", @"暂停", @"触控设置",
                                   @"调试", @"视角", @"副手", @"盾牌"];
    NSArray<NSString *> *keys = @[@"move", @"jump", @"sneak", @"sprint", @"attack",
                                  @"use", @"inventory", @"chat", @"pause", @"settings",
                                  @"debug", @"perspective", @"offhand", @"shield"];
    for (NSInteger i = 0; i < names.count; i++) {
        UIView *card = [self card];
        UILabel *name = [self label:names[i] size:13.0 weight:UIFontWeightSemibold
                              color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
        UIButton *key = [self button:NCLRKeymap.shared.bindings[keys[i]] ?: @"—"
                                action:@selector(editKey:) primary:NO];
        key.tag = i;
        key.titleLabel.font = [UIFont monospacedSystemFontOfSize:14.0 * NCLRKeymap.shared.buttonSize
                                                          weight:UIFontWeightSemibold];
        key.contentHorizontalAlignment = UIControlContentHorizontalAlignmentCenter;
        [self.keyButtons addObject:key];
        [card addSubview:name];
        [card addSubview:key];
        [NSLayoutConstraint activateConstraints:@[
            [name.leadingAnchor constraintEqualToAnchor:card.leadingAnchor constant:16.0],
            [name.centerYAnchor constraintEqualToAnchor:card.centerYAnchor],
            [name.trailingAnchor constraintLessThanOrEqualToAnchor:key.leadingAnchor constant:-12.0],
            [key.trailingAnchor constraintEqualToAnchor:card.trailingAnchor constant:-14.0],
            [key.topAnchor constraintEqualToAnchor:card.topAnchor constant:8.0],
            [key.bottomAnchor constraintEqualToAnchor:card.bottomAnchor constant:-8.0],
            [key.widthAnchor constraintEqualToConstant:128.0],
        ]];
        [self.stack addArrangedSubview:card];
    }

    UILabel *path = [self label:[NSString stringWithFormat:@"保存到：%@", NCLRKeymap.shared.path]
                            size:10.0 weight:UIFontWeightRegular
                           color:NCLRColor(0.50, 0.50, 0.50, 1.0)];
    path.font = [UIFont monospacedSystemFontOfSize:10.0 weight:UIFontWeightRegular];
    [self.stack addArrangedSubview:path];

    UIButton *save = [self button:@"保存" action:@selector(saveKeymap) primary:YES];
    UIButton *exit = [self button:@"退出" action:@selector(exitKeymap) primary:NO];
    UIStackView *actions = [[UIStackView alloc] initWithArrangedSubviews:@[save, exit]];
    actions.axis = UILayoutConstraintAxisHorizontal;
    actions.spacing = 10.0;
    actions.distribution = UIStackViewDistributionFillEqually;
    [self.stack addArrangedSubview:actions];
}

- (void)toggleMenu {
    self.sizeRow.hidden = !self.sizeRow.hidden;
    self.menuToggle.titleLabel.text = self.sizeRow.hidden ? @"触控设置  ▾" : @"触控设置  ▴";
    [self.menuToggle setTitle:(self.sizeRow.hidden ? @"触控设置  ▾" : @"触控设置  ▴")
                     forState:UIControlStateNormal];
}

- (void)sizeChanged:(UISlider *)slider {
    self.sizeValue.text = [NSString stringWithFormat:@"%.2fx", slider.value];
    NCLRKeymap.shared.buttonSize = slider.value;
    for (UIButton *button in self.keyButtons) {
        button.titleLabel.font = [UIFont monospacedSystemFontOfSize:14.0 * slider.value
                                                              weight:UIFontWeightSemibold];
    }
    [self.view setNeedsLayout];
}

- (void)viewDidLayoutSubviews {
    [super viewDidLayoutSubviews];
    if (self.preview.bounds.size.width <= 0.0 || self.preview.bounds.size.height <= 0.0) {
        return;
    }
    self.grid.frame = self.preview.bounds;
    for (NSString *key in self.previewButtons) {
        UIButton *button = self.previewButtons[key];
        NSDictionary *position = NCLRKeymap.shared.positions[key];
        CGFloat x = [position[@"x"] doubleValue];
        CGFloat y = [position[@"y"] doubleValue];
        CGRect area = [self previewContentRect];
        CGFloat size = [position[@"size"] doubleValue] * area.size.height *
            NCLRKeymap.shared.buttonSize;
        size = MAX(26.0, size);
        button.frame = CGRectMake(area.origin.x + x * area.size.width - size / 2.0,
                                  area.origin.y + y * area.size.height - size / 2.0,
                                  size, size);
        button.titleLabel.font = [UIFont monospacedSystemFontOfSize:MAX(9.0, 11.0 * NCLRKeymap.shared.buttonSize)
                                                              weight:UIFontWeightBold];
        BOOL move = [key isEqualToString:@"move"];
        button.layer.cornerRadius = move ? size / 2.0 : 8.0;
        button.layer.borderWidth = move ? 2.0 : ([key isEqualToString:self.selectedKey] ? 2.0 : 0.0);
        button.layer.borderColor = (move ? [UIColor colorWithWhite:1.0 alpha:0.35]
                                         : [UIColor colorWithWhite:0.05 alpha:0.8]).CGColor;
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
    self.selectedKey = key;
    [self.view setNeedsLayout];
}

- (void)updateSelection {
    if (self.selectedKey.length == 0) {
        self.selection.hidden = YES;
        self.resizeHandle.hidden = YES;
        self.selectionInfo.text = @"选择一个按钮后拖动位置，拖右下角圆点调整大小";
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
    NSDictionary *position = NCLRKeymap.shared.positions[self.selectedKey];
    self.selectionInfo.text = [NSString stringWithFormat:
        @"%@  ·  x %.0f%%  y %.0f%%  ·  大小 %.2fx  ·  拖动移动，拖圆点缩放",
        NCLRKeymap.shared.bindings[self.selectedKey] ?: self.selectedKey,
        [position[@"x"] doubleValue] * 100.0,
        [position[@"y"] doubleValue] * 100.0,
        [position[@"size"] doubleValue] * NCLRKeymap.shared.buttonSize];
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
    if (sender.state == UIGestureRecognizerStateBegan) {
        [self selectKey:key];
        self.dragStartCenter = button.center;
    }
    CGPoint translation = [sender translationInView:self.preview];
    CGFloat cx = self.dragStartCenter.x + translation.x;
    CGFloat cy = self.dragStartCenter.y + translation.y;
    CGRect area = [self previewContentRect];
    CGFloat x = MAX(0.025, MIN(0.975, (cx - area.origin.x) / area.size.width));
    CGFloat y = MAX(0.025, MIN(0.975, (cy - area.origin.y) / area.size.height));
    NSDictionary *old = NCLRKeymap.shared.positions[key];
    NCLRKeymap.shared.positions[key] = @{
        @"x" : @(x),
        @"y" : @(y),
        @"size" : old[@"size"] ?: @0.12,
    };
    button.center = CGPointMake(area.origin.x + x * area.size.width,
                                area.origin.y + y * area.size.height);
    if (sender.state == UIGestureRecognizerStateEnded ||
        sender.state == UIGestureRecognizerStateCancelled) {
        [self.view setNeedsLayout];
    }
}

- (void)resizeKey:(UIPanGestureRecognizer *)sender {
    if (self.selectedKey.length == 0) {
        return;
    }
    UIButton *button = self.previewButtons[self.selectedKey];
    if (button == nil) {
        return;
    }
    CGPoint point = [sender locationInView:self.preview];
    CGFloat distance = hypot(point.x - button.center.x, point.y - button.center.y);
    CGFloat size = MAX(28.0, distance * 2.0);
    CGRect area = [self previewContentRect];
    CGFloat normalized = size / MAX(1.0, area.size.height * NCLRKeymap.shared.buttonSize);
    normalized = MAX(0.045, MIN(0.45, normalized));
    NSDictionary *old = NCLRKeymap.shared.positions[self.selectedKey];
    NCLRKeymap.shared.positions[self.selectedKey] = @{
        @"x" : old[@"x"] ?: @0.5,
        @"y" : old[@"y"] ?: @0.5,
        @"size" : @(normalized),
    };
    [self.view setNeedsLayout];
}

- (void)toggleGrid {
    self.grid.hidden = !self.grid.hidden;
}

- (void)resetLayoutTapped {
    [NCLRKeymap.shared resetLayout];
    [self.view setNeedsLayout];
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
        [NCLRKeymap.shared addKey:picked label:picked];
        [NCLRKeymap.shared save];
        UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"已添加"
                                                                       message:@"退出触控编辑再进入即可看到新按键。"
                                                                preferredStyle:UIAlertControllerStyleAlert];
        [alert addAction:[UIAlertAction actionWithTitle:@"好" style:UIAlertActionStyleDefault handler:nil]];
        [self presentViewController:alert animated:YES completion:nil];
    }];
}
- (void)editKey:(UIButton *)sender {
    NSArray<NSString *> *keys = @[@"move", @"jump", @"sneak", @"sprint", @"attack",
                                  @"use", @"inventory", @"chat", @"pause", @"settings",
                                  @"debug", @"perspective", @"offhand", @"shield"];
    NSArray<NSString *> *names = @[@"移动摇杆", @"跳跃", @"潜行", @"冲刺", @"攻击",
                                   @"使用", @"背包", @"聊天", @"暂停", @"触控设置",
                                   @"调试", @"视角", @"副手", @"盾牌"];
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
        self.previewHeight.constant = self.canvasFullscreen ? MAX(300.0, height - 100.0)
                                                            : MAX(300.0, MIN(640.0, height * 0.58));
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
    return UIEdgeInsetsInsetRect(self.preview.bounds, self.view.safeAreaInsets);
}

- (void)toggleCanvas {
    self.canvasFullscreen = !self.canvasFullscreen;
    for (UIView *view in self.stack.arrangedSubviews) {
        view.hidden = self.canvasFullscreen && view != self.previewCard;
    }
    self.canvasClose.hidden = !self.canvasFullscreen;
    self.scroll.scrollEnabled = !self.canvasFullscreen;
    if (self.canvasFullscreen) {
        self.previewCard.backgroundColor = UIColor.blackColor;
        self.previewCard.layer.cornerRadius = 0.0;
        self.previewCard.layer.borderWidth = 0.0;
        self.preview.backgroundColor = UIColor.blackColor;
        self.preview.layer.cornerRadius = 0.0;
        self.preview.layer.borderWidth = 0.0;
        self.stackLeading.constant = 0.0;
        self.stackTrailing.constant = 0.0;
        self.previewHeight.constant = MAX(300.0, self.view.bounds.size.height -
            self.view.safeAreaInsets.top - self.view.safeAreaInsets.bottom - 24.0);
    } else {
        self.previewCard.backgroundColor = NCLRColor(0.10, 0.10, 0.10, 1.0);
        self.previewCard.layer.cornerRadius = 16.0;
        self.previewCard.layer.borderWidth = 1.0;
        self.preview.backgroundColor = NCLRColor(0.02, 0.02, 0.02, 1.0);
        self.preview.layer.cornerRadius = 12.0;
        self.preview.layer.borderWidth = 1.0;
        self.stackLeading.constant = 24.0;
        self.stackTrailing.constant = -24.0;
        self.previewHeight.constant = MAX(300.0, MIN(640.0, self.view.bounds.size.height * 0.58));
    }
    [self.view setNeedsLayout];
    [self.view layoutIfNeeded];
}
- (void)saveKeymap {
    NCLRKeymap.shared.buttonSize = self.sizeSlider.value;
    NCLRKeymap.shared.opacity = self.opacitySlider.value;
    NCLRKeymap.shared.touchMode = @[@"crosshair", @"tap", @"dpad"][MAX(0, self.modeControl.selectedSegmentIndex)];
    NCLRKeymap.shared.dynamicJoystick = self.dynamicSwitch.on;
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

@interface NCLRRootController : UIViewController <UITableViewDataSource, UITableViewDelegate>
@property (nonatomic, strong) UIView *sidebar;
@property (nonatomic, strong) UIView *content;
@property (nonatomic, strong) UIView *compactBar;
@property (nonatomic, strong) NSArray<UIButton *> *navButtons;
@property (nonatomic, strong) NSArray<UIButton *> *compactButtons;
@property (nonatomic, strong) NSArray *regularConstraints;
@property (nonatomic, strong) NSArray *compactConstraints;
@property (nonatomic, strong) UILabel *accountLabel;
@property (nonatomic, assign) NSInteger page;
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
    self.view.backgroundColor = NCLRColor(0.02, 0.02, 0.02, 1.0);
    CAGradientLayer *gradient = [CAGradientLayer layer];
    gradient.colors = @[
        (__bridge id)NCLRColor(0.07, 0.07, 0.07, 1.0).CGColor,
        (__bridge id)NCLRColor(0.02, 0.02, 0.02, 1.0).CGColor,
        (__bridge id)NCLRColor(0.10, 0.10, 0.10, 1.0).CGColor,
    ];
    gradient.startPoint = CGPointMake(0.1, 0.0);
    gradient.endPoint = CGPointMake(0.9, 1.0);
    [self.view.layer insertSublayer:gradient atIndex:0];

    self.sidebar = [[UIView alloc] init];
    self.sidebar.translatesAutoresizingMaskIntoConstraints = NO;
    self.sidebar.backgroundColor = NCLRColor(0.02, 0.02, 0.02, 0.88);
    self.sidebar.layer.borderWidth = 1.0;
    self.sidebar.layer.borderColor = [UIColor colorWithWhite:1.0 alpha:0.06].CGColor;
    [self.view addSubview:self.sidebar];

    self.content = [[UIView alloc] init];
    self.content.translatesAutoresizingMaskIntoConstraints = NO;
    [self.view addSubview:self.content];

    // Compact widths (iPhone, iPad split view and resizable windows) get a
    // horizontal navigation bar instead of the 228 pt sidebar.
    self.compactBar = [[UIView alloc] init];
    self.compactBar.translatesAutoresizingMaskIntoConstraints = NO;
    self.compactBar.backgroundColor = NCLRColor(0.02, 0.02, 0.02, 0.96);
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
    NSArray *compactNames = @[@"主页", @"版本", @"模组", @"光影", @"资源包", @"账号", @"设置"];
    for (NSInteger i = 0; i < compactNames.count; i++) {
        UIButton *button = [UIButton buttonWithType:UIButtonTypeSystem];
        button.tag = i;
        button.translatesAutoresizingMaskIntoConstraints = NO;
        button.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.035];
        button.layer.cornerRadius = 9.0;
        button.contentHorizontalAlignment = UIControlContentHorizontalAlignmentLeft;
        button.contentEdgeInsets = UIEdgeInsetsMake(0, 14, 0, 8);
        button.titleLabel.font = [UIFont systemFontOfSize:13.0 weight:UIFontWeightSemibold];
        button.titleLabel.lineBreakMode = NSLineBreakByClipping;
        NSArray *glyphs = @[@"⌂", @"▤", @"⧉", @"☀", @"▦", @"●", @"⚙"];
        [button setTitle:[NSString stringWithFormat:@"%@  %@", glyphs[i], compactNames[i]]
                forState:UIControlStateNormal];
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
    NSArray *names = @[@"主页", @"版本", @"模组", @"光影", @"资源包", @"账号", @"设置"];
    NSArray *symbols = @[@"house.fill", @"square.stack.3d.up.fill", @"puzzlepiece.extension.fill",
                         @"sun.max.fill", @"square.grid.2x2.fill",
                         @"person.crop.circle.fill", @"gearshape.fill"];
    NSMutableArray *buttons = [NSMutableArray array];
    for (NSInteger i = 0; i < names.count; i++) {
        UIButton *button = [UIButton buttonWithType:UIButtonTypeSystem];
        button.tag = i;
        button.translatesAutoresizingMaskIntoConstraints = NO;
        button.contentHorizontalAlignment = UIControlContentHorizontalAlignmentLeft;
        button.contentEdgeInsets = UIEdgeInsetsMake(0, 14, 0, 8);
        button.titleLabel.font = [UIFont systemFontOfSize:13.0 weight:UIFontWeightSemibold];
        button.titleLabel.lineBreakMode = NSLineBreakByClipping;
        NSArray *glyphs = @[@"⌂", @"▤", @"⧉", @"☀", @"▦", @"●", @"⚙"];
        [button setTitle:[NSString stringWithFormat:@"%@  %@", glyphs[i], names[i]]
                forState:UIControlStateNormal];
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
    account.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.045];
    account.layer.cornerRadius = 12.0;
    UILabel *avatar = [self label:@"N" size:15.0 weight:UIFontWeightBold
                            color:NCLRColor(0.95, 0.95, 0.95, 1.0)];
    avatar.textAlignment = NSTextAlignmentCenter;
    avatar.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.06];
    avatar.layer.cornerRadius = 16.0;
    avatar.layer.masksToBounds = YES;
    avatar.translatesAutoresizingMaskIntoConstraints = NO;
    self.accountLabel = [self label:@"未登录" size:11.0 weight:UIFontWeightSemibold
                              color:NCLRColor(0.94, 0.94, 0.94, 1.0)];
    UILabel *accountDetail = [self label:@"Microsoft 账号" size:9.0 weight:UIFontWeightRegular
                                    color:NCLRColor(0.50, 0.50, 0.50, 1.0)];
    [account addSubview:avatar];
    [account addSubview:self.accountLabel];
    [account addSubview:accountDetail];
    [NSLayoutConstraint activateConstraints:@[
        [avatar.leadingAnchor constraintEqualToAnchor:account.leadingAnchor constant:10.0],
        [avatar.topAnchor constraintEqualToAnchor:account.topAnchor constant:10.0],
        [avatar.widthAnchor constraintEqualToConstant:32.0],
        [avatar.heightAnchor constraintEqualToConstant:32.0],
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

    [self showPage:0];
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
    [self showPage:5];
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
        button.alpha = selected ? 1.0 : 0.82;
    }
    for (NSInteger i = 0; i < self.compactButtons.count; i++) {
        UIButton *button = self.compactButtons[i];
        BOOL selected = i == page;
        button.backgroundColor = selected ? [UIColor colorWithWhite:1.0 alpha:0.075] : UIColor.clearColor;
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
        case 5: controller = [[NCLRAccountController alloc] init]; break;
        case 6: controller = [[NCLRSettingsController alloc] init]; break;
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
    [self refreshAccount];
}

- (void)refreshAccount {
    NCLRAuth *auth = NCLRAuth.shared;
    BOOL premium = auth.developerPremium || auth.token.length > 0;
    self.accountLabel.text = premium ? (auth.name ?: @"正版用户") : @"未登录";
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
    self.window = [[UIWindow alloc] initWithWindowScene:windowScene];
    self.window.rootViewController = [[NCLRRootController alloc] init];
    self.window.windowLevel = UIWindowLevelNormal;
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

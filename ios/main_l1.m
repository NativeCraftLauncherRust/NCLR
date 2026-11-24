#import <UIKit/UIKit.h>
#import <Security/Security.h>
#import <CommonCrypto/CommonDigest.h>
#import <AuthenticationServices/AuthenticationServices.h>
#import <SafariServices/SafariServices.h>
#import <stdlib.h>
#include <string.h>

extern int horizon_ios_run(const char *resources, const char *game_dir);
@class NCLRResourcePreparer;
@interface NCLRResourcePreparer : NSObject
+ (instancetype)shared;
- (void)prepareWithProgress:(void (^)(NSString *line))progress
                 completion:(void (^)(NSString *error))completion;
@end
extern const char *horizon_ios_assemble(const char *bundle, const char *client_jar,
                                        const char *index, const char *store, const char *out);


static NSString *const kNCLRService = @"dev.zrxmaxlnx.nclr.launcher";
static NSString *const kNCLRAccount = @"minecraft-access-token";
static NSString *const kNCLRClientKey = @"NCLRMicrosoftClientID";
static NSString *const kNCLRDefaultClientID = @"da723f82-1e03-48ef-b3ae-313b2e4c34ca";
static NSString *const kNCLRAccountNameKey = @"NCLRAccountName";

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
        CGFloat hi = MAX(r, MAX(g, b));
        CGFloat lo = MIN(r, MIN(g, b));
        if (hi - lo <= 0.10 || hi <= 0.12) {
            r = 1.0 - r; g = 1.0 - g; b = 1.0 - b;
        }
    }
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
@property (nonatomic, assign) BOOL leftHanded;
@property (nonatomic, assign) BOOL joystickSprint;
@property (nonatomic, assign) CGFloat sensitivity;
@property (nonatomic, assign) CGFloat joystickDeadzone;
@property (nonatomic, assign) BOOL sprintButton;
@property (nonatomic, assign) NSInteger buttonStyle;
@property (nonatomic, assign) BOOL iconLabels;
@property (nonatomic, strong) NSMutableSet<NSString *> *hiddenKeys;
+ (instancetype)shared;
- (BOOL)isHidden:(NSString *)key;
- (void)setHidden:(BOOL)hidden forKey:(NSString *)key;
- (void)applyPreset:(NSString *)preset;
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
    self.leftHanded = NO;
    self.joystickSprint = NO;
    self.sensitivity = 1.0;
    self.joystickDeadzone = 0.16;
    self.sprintButton = YES;
    self.buttonStyle = 0;
    self.iconLabels = NO;
    self.hiddenKeys = [NSMutableSet setWithArray:@[@"settings", @"debug", @"perspective", @"offhand", @"shield", @"keyboard"]];
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
    NSNumber *sensitivity = json[@"sensitivity"];
    if ([sensitivity isKindOfClass:[NSNumber class]]) {
        self.sensitivity = MAX(0.3, MIN(2.5, sensitivity.doubleValue));
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
        @"joystickDeadzone" : @(self.joystickDeadzone),
        @"sprintButton" : @(self.sprintButton),
        @"buttonStyle" : @(self.buttonStyle),
        @"iconLabels" : @(self.iconLabels),
        @"hidden" : self.hiddenKeys.allObjects ?: @[],
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
@property (nonatomic, copy) NSString *token;
@property (nonatomic, copy) NSString *name;
@property (nonatomic, assign) BOOL developerPremium;
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
        _developerPremium = NO;
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

- (void)fetchProfileWithToken:(NSString *)token completion:(NCLRAuthCompletion)completion {
    __weak typeof(self) weakSelf = self;
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
            dispatch_async(dispatch_get_main_queue(), ^{
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
    if (NCLRAuth.shared.token.length == 0) {
        UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"需要正版登录"
                                                                       message:@"首次准备游戏资源前，请先在“账号”页登录 Microsoft 正版账号。"
                                                                preferredStyle:UIAlertControllerStyleAlert];
        [alert addAction:[UIAlertAction actionWithTitle:@"好" style:UIAlertActionStyleDefault handler:nil]];
        [self presentViewController:alert animated:YES completion:nil];
        return;
    }
    NSString *resources = [NCLRDocuments() stringByAppendingPathComponent:@"horizon-client/26.3"];
    if (![[NSFileManager defaultManager] fileExistsAtPath:
            [resources stringByAppendingPathComponent:@"horizon-resources.json"]]) {
        [self prepareResources];
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

- (void)prepareResources {
    UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"首次启动准备"
                                                                   message:@"正在从 Mojang 官方源下载 26.3 并组装…"
                                                            preferredStyle:UIAlertControllerStyleAlert];
    [self presentViewController:alert animated:YES completion:nil];
    [[NCLRResourcePreparer shared] prepareWithProgress:^(NSString *line) {
        alert.message = line;
    } completion:^(NSString *error) {
        [alert dismissViewControllerAnimated:YES completion:^{
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
    }];
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

    UILabel *legal = [self label:@"启动器只在你主动登录时联系 Microsoft / Minecraft Services。使用公开的 Azure Client ID，不提供离线绕过。"
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
        window.rootViewController = [[rootClass alloc] init];
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
    self.visibilitySwitches = [NSMutableDictionary dictionary];
    self.previewButtons = [NSMutableDictionary dictionary];
    [self setupPage];
    [self.stack addArrangedSubview:[self headerWithTitle:@"触控界面"
                                                subtitle:@"游戏内触控按钮 · 布局、位置与大小"
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
    UIButton *addKey = [self button:@"+ 添加触控键" action:@selector(addKeyTapped) primary:NO];
    UIButton *gridButton = [self button:@"网格" action:@selector(toggleGrid) primary:NO];
    UIButton *resetButton = [self button:@"重置布局" action:@selector(resetLayoutTapped) primary:NO];
    UIButton *fullButton = [self button:@"全屏编辑" action:@selector(toggleCanvas) primary:NO];
    UIButton *exportButton = [self button:@"导出" action:@selector(exportKeymap) primary:NO];
    UIButton *importButton = [self button:@"导入" action:@selector(importKeymap) primary:NO];
    UIStackView *ioRow = [[UIStackView alloc] initWithArrangedSubviews:@[exportButton, importButton]];
    ioRow.axis = UILayoutConstraintAxisHorizontal;
    ioRow.spacing = 8.0;
    ioRow.distribution = UIStackViewDistributionFillEqually;
    UIStackView *tools = [[UIStackView alloc] initWithArrangedSubviews:@[gridButton, resetButton, fullButton]];
    tools.axis = UILayoutConstraintAxisHorizontal;
    tools.spacing = 8.0;
    tools.distribution = UIStackViewDistributionFillEqually;
    UIStackView *menuStack = [[UIStackView alloc] initWithArrangedSubviews:
        @[self.menuToggle, self.sizePresetControl, self.presetControl, self.styleControl, self.sizeRow, opacityRow, self.modeControl, dynamicRow, leftRow, sprintRow, sensitivityRow, deadzoneRow, sprintButtonRow, iconLabelsRow, addKey, tools, ioRow]];
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
        [self.preview.centerXAnchor constraintEqualToAnchor:previewCard.centerXAnchor],
        [self.preview.topAnchor constraintEqualToAnchor:previewLabel.bottomAnchor constant:8.0],
        [self.preview.widthAnchor constraintLessThanOrEqualToAnchor:previewCard.widthAnchor constant:-24.0],
        [self.preview.bottomAnchor constraintLessThanOrEqualToAnchor:previewCard.bottomAnchor constant:-12.0],
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
                                   @"调试", @"视角", @"副手", @"盾牌", @"系统键盘"];
    NSArray<NSString *> *keys = @[@"move", @"jump", @"sneak", @"sprint", @"attack",
                                  @"use", @"inventory", @"chat", @"pause", @"settings",
                                  @"debug", @"perspective", @"offhand", @"shield", @"keyboard"];
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
        UISwitch *vis = [[UISwitch alloc] init];
        vis.on = ![NCLRKeymap.shared isHidden:keys[i]];
        vis.tag = i;
        [vis addTarget:self action:@selector(visibilityChanged:) forControlEvents:UIControlEventValueChanged];
        self.visibilitySwitches[keys[i]] = vis;
        [card addSubview:name];
        [card addSubview:key];
        [card addSubview:vis];
        [NSLayoutConstraint activateConstraints:@[
            [name.leadingAnchor constraintEqualToAnchor:card.leadingAnchor constant:16.0],
            [name.centerYAnchor constraintEqualToAnchor:card.centerYAnchor],
            [name.trailingAnchor constraintLessThanOrEqualToAnchor:key.leadingAnchor constant:-10.0],
            [key.trailingAnchor constraintEqualToAnchor:vis.leadingAnchor constant:-10.0],
            [key.centerYAnchor constraintEqualToAnchor:card.centerYAnchor],
            [key.widthAnchor constraintEqualToConstant:96.0],
            [key.heightAnchor constraintEqualToConstant:36.0],
            [vis.trailingAnchor constraintEqualToAnchor:card.trailingAnchor constant:-14.0],
            [vis.centerYAnchor constraintEqualToAnchor:card.centerYAnchor],
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
        if (position == nil) position = @{ @"x" : @0.5, @"y" : @0.5, @"size" : @0.08 };
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
        button.alpha = [NCLRKeymap.shared isHidden:key] ? 0.22 : 1.0;
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
    UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"重置布局"
                                                                   message:@"当前预设的位置和显隐会被恢复为默认值。"
                                                            preferredStyle:UIAlertControllerStyleAlert];
    [alert addAction:[UIAlertAction actionWithTitle:@"取消" style:UIAlertActionStyleCancel handler:nil]];
    [alert addAction:[UIAlertAction actionWithTitle:@"重置" style:UIAlertActionStyleDestructive
                                            handler:^(UIAlertAction *action) {
        [NCLRKeymap.shared resetLayout];
        [NCLRKeymap.shared save];
        [self.view setNeedsLayout];
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
    [self.view setNeedsLayout];
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
- (NSData *)download:(NSString *)url error:(NSString **)error {
    NSError *last = nil;
    for (int attempt = 0; attempt < 3; attempt++) {
        NSData *data = [NSData dataWithContentsOfURL:[NSURL URLWithString:url]
                                             options:0 error:&last];
        if (data.length > 0) return data;
        [NSThread sleepForTimeInterval:1.0 + attempt];
    }
    if (error) *error = [NSString stringWithFormat:@"下载失败：%@（%@）", url, last.localizedDescription ?: @"无响应"];
    return nil;
}
- (void)prepareWithProgress:(void (^)(NSString *))progress
                 completion:(void (^)(NSString *))completion {
    dispatch_async(_queue, ^{
        void (^report)(NSString *) = ^(NSString *line) {
            dispatch_async(dispatch_get_main_queue(), ^{ if (progress) progress(line); });
        };
        NSString *cache = [NCLRDocuments() stringByAppendingPathComponent:@".nclr-cache"];
        NSString *jarPath = [cache stringByAppendingPathComponent:@"client.jar"];
        NSString *indexPath = [cache stringByAppendingPathComponent:@"index.json"];
        NSString *objects = [cache stringByAppendingPathComponent:@"objects"];
        NSString *out = [NCLRDocuments() stringByAppendingPathComponent:@"horizon-client/26.3"];
        NCLRCreateDirectory(cache);
        NCLRCreateDirectory(objects);
        NSString *error = nil;

        report(@"读取 Mojang 版本清单…");
        NSData *manifestData = [self download:@"https://piston-meta.mojang.com/mc/game/version_manifest_v2.json" error:&error];
        if (manifestData == nil) { completion(error); return; }
        NSDictionary *manifest = [NSJSONSerialization JSONObjectWithData:manifestData options:0 error:nil];
        NSString *versionURL = nil;
        for (NSDictionary *v in manifest[@"versions"]) {
            if ([v[@"id"] isEqualToString:@"26.3"]) { versionURL = v[@"url"]; break; }
        }
        if (versionURL == nil) { completion(@"Mojang 清单里没有 26.3"); return; }

        report(@"读取 26.3 版本信息…");
        NSData *versionData = [self download:versionURL error:&error];
        if (versionData == nil) { completion(error); return; }
        NSDictionary *version = [NSJSONSerialization JSONObjectWithData:versionData options:0 error:nil];
        NSDictionary *client = version[@"downloads"][@"client"];
        NSDictionary *assets = version[@"assetIndex"];
        if (client[@"url"] == nil || assets[@"url"] == nil) { completion(@"版本信息缺少 client/assetIndex"); return; }

        report(@"下载 client.jar…");
        NSData *jar = [self download:client[@"url"] error:&error];
        if (jar == nil) { completion(error); return; }
        if (![NCLRSHA1Hex(jar) isEqualToString:client[@"sha1"]]) { completion(@"client.jar SHA-1 校验失败"); return; }
        [jar writeToFile:jarPath atomically:YES];

        report(@"下载资源索引…");
        NSData *indexData = [self download:assets[@"url"] error:&error];
        if (indexData == nil) { completion(error); return; }
        if (![NCLRSHA1Hex(indexData) isEqualToString:assets[@"sha1"]]) { completion(@"asset index SHA-1 校验失败"); return; }
        [indexData writeToFile:indexPath atomically:YES];
        NSDictionary *index = [NSJSONSerialization JSONObjectWithData:indexData options:0 error:nil];
        NSDictionary *objectsMap = index[@"objects"];
        NSArray *names = [objectsMap.allKeys sortedArrayUsingSelector:@selector(compare:)];
        // 4,900 small objects: a serial loop is dominated by round trips. Eight
        // workers keep the first run practical on a phone.
        NSOperationQueue *assetQueue = [[NSOperationQueue alloc] init];
        assetQueue.maxConcurrentOperationCount = 8;
        NSLock *state = [[NSLock alloc] init];
        __block NSInteger done = 0;
        __block NSString *assetError = nil;
        for (NSString *name in names) {
            [assetQueue addOperationWithBlock:^{
                [state lock];
                if (assetError != nil) { [state unlock]; return; }
                [state unlock];
                NSDictionary *object = objectsMap[name];
                NSString *hash = object[@"hash"];
                if (hash.length < 2) return;
                NSString *folder = [objects stringByAppendingPathComponent:[hash substringToIndex:2]];
                NCLRCreateDirectory(folder);
                NSString *file = [folder stringByAppendingPathComponent:hash];
                if ([[NSFileManager defaultManager] fileExistsAtPath:file]) {
                    [state lock]; done++; [state unlock];
                    return;
                }
                NSString *url = [NSString stringWithFormat:@"https://resources.download.minecraft.net/%@/%@",
                                 [hash substringToIndex:2], hash];
                NSString *oneError = nil;
                NSData *data = [self download:url error:&oneError];
                if (data == nil || ![NCLRSHA1Hex(data) isEqualToString:hash]) {
                    [state lock];
                    if (assetError == nil) assetError = oneError ?: [NSString stringWithFormat:@"资源校验失败：%@", name];
                    [state unlock];
                    [assetQueue cancelAllOperations];
                    return;
                }
                [data writeToFile:file atomically:YES];
                NSInteger current;
                [state lock]; current = ++done; [state unlock];
                if (current % 50 == 0) {
                    report([NSString stringWithFormat:@"下载资源 %ld/%lu…", (long)current, (unsigned long)names.count]);
                }
            }];
        }
        [assetQueue waitUntilAllOperationsAreFinished];
        [state lock]; NSString *finalAssetError = assetError; [state unlock];
        if (finalAssetError != nil) { completion(finalAssetError); return; }

        NSString *bundle = [[NSBundle mainBundle] pathForResource:@"ship" ofType:@"hzr3"];
        if (bundle.length == 0) { completion(@"IPA 内缺少 ship.hzr3"); return; }
        report(@"组装游戏资源…");
        [[NSFileManager defaultManager] removeItemAtPath:out error:nil];
        NCLRCreateDirectory([out stringByDeletingLastPathComponent]);
        const char *message = horizon_ios_assemble(bundle.UTF8String, jarPath.UTF8String,
                                                   indexPath.UTF8String, objects.UTF8String,
                                                   out.UTF8String);
        if (message != NULL) {
            completion([NSString stringWithUTF8String:message]);
            return;
        }
        // hzr-device assembles models/data but not the optional 360 MB sound
        // store; copy the official sounds into the finished tree as well.
        report(@"复制原版音效…");
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
            [[NSFileManager defaultManager] copyItemAtPath:source toPath:dest error:nil];
        }
        if (![[NSFileManager defaultManager] fileExistsAtPath:
                [out stringByAppendingPathComponent:@"horizon-resources.json"]]) {
            completion(@"组装完成但资源清单不存在");
            return;
        }
        report(@"资源准备完成");
        completion(nil);
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
    BOOL premium = auth.token.length > 0;
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

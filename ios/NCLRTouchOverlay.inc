// Included after NCLRKeymap so the shell and editor share saved layouts.
extern uint64_t horizon_ios_touch_status(void);
extern uint64_t horizon_ios_touch_revision(void);
extern size_t horizon_ios_touch_layout(uint8_t *output, size_t capacity);
extern void horizon_ios_touch_visuals(const char *snapshot);
extern void horizon_ios_touch_look(double dx, double dy);
extern void horizon_ios_touch_key(uint16_t usage, int pressed);
extern void horizon_ios_touch_button(uint8_t button, int pressed);
extern void horizon_ios_touch_reset(void);
extern void horizon_ios_touch_keyboard(void);

static uint16_t NCLRTouchUsage(NSString *label) {
    NSString *key = label.uppercaseString;
    if (key.length == 1) {
        unichar ch = [key characterAtIndex:0];
        if (ch >= 'A' && ch <= 'Z') return 0x04 + ch - 'A';
        if (ch >= '1' && ch <= '9') return 0x1E + ch - '1';
        if (ch == '0') return 0x27;
    }
    if ([key hasPrefix:@"F"] && key.length <= 3) {
        NSInteger number = [[key substringFromIndex:1] integerValue];
        if (number >= 1 && number <= 12) return 0x39 + number;
    }
    NSDictionary *usages = @{@"SPACE": @0x2C, @"ENTER": @0x28, @"ESC": @0x29,
        @"ESCAPE": @0x29, @"TAB": @0x2B, @"BACKSPACE": @0x2A, @"SHIFT": @0xE1,
        @"CTRL": @0xE0, @"CONTROL": @0xE0, @"ALT": @0xE2, @"SUPER": @0xE3,
        @"UP": @0x52, @"DOWN": @0x51, @"LEFT": @0x50, @"RIGHT": @0x4F,
        @"INSERT": @0x49, @"HOME": @0x4A, @"PAGEUP": @0x4B, @"DELETE": @0x4C,
        @"END": @0x4D, @"PAGEDOWN": @0x4E,
        @"-": @0x2D, @"=": @0x2E, @"[": @0x2F, @"]": @0x30, @"\\": @0x31,
        @";": @0x33, @"'": @0x34, @"`": @0x35, @",": @0x36, @".": @0x37,
        @"/": @0x38, @"ACTION:KEY.SWAPOFFHAND": @0x09};
    return [usages[key] unsignedShortValue];
}

static void NCLRTouchSend(NSString *label, BOOL down) {
    NSArray *mouse = @[@"MouseLeft", @"MouseRight", @"MouseMiddle"];
    NSUInteger button = [mouse indexOfObject:label];
    if (button != NSNotFound) {
        horizon_ios_touch_button((uint8_t)button, down);
    } else {
        uint16_t usage = NCLRTouchUsage(label);
        if (usage) horizon_ios_touch_key(usage, down);
    }
}

@interface NCLRTouchFinger : NSObject
@property (nonatomic, copy) NSString *key;
@property (nonatomic, copy) NSString *label;
@property (nonatomic, assign) CGPoint anchor;
@property (nonatomic, assign) CGPoint previous;
@property (nonatomic, assign) NSTimeInterval began;
@property (nonatomic, assign) BOOL dragged;
@property (nonatomic, assign) BOOL held;
@property (nonatomic, assign) BOOL swipe;
@end
@implementation NCLRTouchFinger
@end

@interface NCLRTouchOverlay : UIView
@property (nonatomic, strong) NSMapTable<UITouch *, NCLRTouchFinger *> *fingers;
@property (nonatomic, strong) NSCountedSet<NSString *> *heldLabels;
@property (nonatomic, strong) NSMutableSet<NSString *> *latchedKeys;
@property (nonatomic, strong) NSMutableSet<NSString *> *moveLabels;
@property (nonatomic, strong) NSArray<NSString *> *visibleKeys;
@property (nonatomic, assign) NSInteger mode;
@property (nonatomic, assign) CGSize guiSize;
@property (nonatomic, assign) CGSize previousSize;
@property (nonatomic, strong) CADisplayLink *display;
@property (nonatomic, strong) NSDictionary *layout;
@property (nonatomic, strong) NSDictionary<NSString *, NSDictionary *> *buttons;
@property (nonatomic, assign) uint64_t layoutRevision;
@property (nonatomic, copy) NSData *lastVisuals;
+ (instancetype)shared;
- (void)start;
- (void)stop;
- (void)cancelInput;
@end

@implementation NCLRTouchOverlay
+ (instancetype)shared {
    static NCLRTouchOverlay *overlay;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ overlay = [[self alloc] initWithFrame:CGRectZero]; });
    return overlay;
}

- (instancetype)initWithFrame:(CGRect)frame {
    if ((self = [super initWithFrame:frame])) {
        self.backgroundColor = UIColor.clearColor;
        self.opaque = NO;
        self.multipleTouchEnabled = YES;
        self.fingers = [NSMapTable strongToStrongObjectsMapTable];
        self.heldLabels = [NSCountedSet set];
        self.latchedKeys = [NSMutableSet set];
        self.moveLabels = [NSMutableSet set];
        self.visibleKeys = @[];
        self.autoresizingMask = UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
    }
    return self;
}

- (void)start {
    [self stop];
    self.layoutRevision = 0;
    self.layout = nil;
    self.buttons = @{};
    self.visibleKeys = @[];
    [[NSNotificationCenter defaultCenter] addObserver:self selector:@selector(becameInactive:)
        name:UIApplicationWillResignActiveNotification object:nil];
    [[NSNotificationCenter defaultCenter] addObserver:self selector:@selector(becameInactive:)
        name:UISceneWillDeactivateNotification object:nil];
    self.display = [CADisplayLink displayLinkWithTarget:self selector:@selector(tick:)];
    [self.display addToRunLoop:NSRunLoop.mainRunLoop forMode:NSRunLoopCommonModes];
}

- (void)stop {
    [self.display invalidate];
    self.display = nil;
    [[NSNotificationCenter defaultCenter] removeObserver:self];
    [self cancelInput];
    self.mode = 0;
    [self removeFromSuperview];
}

- (void)cancelInput {
    // Reset one source. Physical keys and mouse buttons retain their ownership.
    horizon_ios_touch_reset();
    [self.fingers removeAllObjects];
    [self.heldLabels removeAllObjects];
    [self.latchedKeys removeAllObjects];
    [self.moveLabels removeAllObjects];
    [self publishVisuals];
}

- (void)becameInactive:(NSNotification *)notification {
    [self cancelInput];
    (void)notification;
}

- (void)holdLabel:(NSString *)label down:(BOOL)down {
    if (label.length == 0) return;
    NSUInteger count = [self.heldLabels countForObject:label];
    if (down) {
        [self.heldLabels addObject:label];
        if (count == 0) NCLRTouchSend(label, YES);
    } else if (count > 0) {
        [self.heldLabels removeObject:label];
        if (count == 1) NCLRTouchSend(label, NO);
    }
}

- (void)loadSharedLayout {
    uint64_t revision = horizon_ios_touch_revision();
    if (revision == self.layoutRevision) return;
    size_t needed = horizon_ios_touch_layout(NULL, 0);
    if (needed < 2 || needed > 1024 * 1024) return;
    NSMutableData *data = [NSMutableData dataWithLength:needed];
    if (horizon_ios_touch_layout(data.mutableBytes, needed) != needed) return;
    [data setLength:needed - 1];
    NSDictionary *layout = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    if (![layout isKindOfClass:NSDictionary.class]) return;
    self.layout = layout;
    self.layoutRevision = revision;
    NSMutableDictionary *buttons = [NSMutableDictionary dictionary];
    NSMutableArray *keys = [NSMutableArray array];
    for (NSDictionary *button in layout[@"buttons"]) {
        NSString *key = button[@"id"];
        if (key.length == 0) continue;
        buttons[key] = button;
        [keys addObject:key];
    }
    self.buttons = buttons;
    self.visibleKeys = keys;
}

- (CGRect)rectForKey:(NSString *)key {
    NSArray *rect = self.buttons[key][@"rect"];
    if (rect.count != 4) return CGRectZero;
    return CGRectMake([rect[0] doubleValue] * self.bounds.size.width,
                      [rect[1] doubleValue] * self.bounds.size.height,
                      [rect[2] doubleValue] * self.bounds.size.width,
                      [rect[3] doubleValue] * self.bounds.size.height);
}

- (void)publishVisuals {
    NSMutableSet *pressed = [self.latchedKeys mutableCopy];
    NSArray *anchor = nil;
    NSArray *movement = @[@0, @0];
    NSArray *pointer = nil;
    for (NCLRTouchFinger *finger in self.fingers.objectEnumerator) {
        if (self.buttons[finger.key]) [pressed addObject:finger.key];
        if ([finger.key isEqual:@"move"]) {
            CGFloat radius = MAX(1, [self rectForKey:@"move"].size.height / 2);
            CGFloat x = (finger.previous.x - finger.anchor.x) / radius;
            CGFloat y = (finger.anchor.y - finger.previous.y) / radius;
            CGFloat length = MAX(1, hypot(x, y));
            movement = @[@(x / length), @(y / length)];
            anchor = @[@(finger.anchor.x / MAX(1, self.bounds.size.width)),
                       @(finger.anchor.y / MAX(1, self.bounds.size.height))];
        } else if ([finger.key isEqual:@"look"] && ![self.layout[@"crosshair"] boolValue]) {
            pointer = @[@(finger.previous.x / MAX(1, self.bounds.size.width)),
                        @(finger.previous.y / MAX(1, self.bounds.size.height))];
        }
    }
    NSDictionary *snapshot = @{@"pressed": [pressed.allObjects sortedArrayUsingSelector:@selector(compare:)],
        @"move": movement, @"anchor": anchor ?: NSNull.null, @"pointer": pointer ?: NSNull.null};
    NSData *data = [NSJSONSerialization dataWithJSONObject:snapshot options:NSJSONWritingSortedKeys error:nil];
    if (![data isEqual:self.lastVisuals]) {
        self.lastVisuals = data;
        NSString *json = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
        horizon_ios_touch_visuals(json.UTF8String);
    }
}

- (void)tick:(CADisplayLink *)display {
    uint64_t status = horizon_ios_touch_status();
    NSInteger mode = status & 0xFF;
    if (UIApplication.sharedApplication.applicationState != UIApplicationStateActive) mode = 0;
    UIView *target = [[NCLRHardwareInput shared] pointerTargetView];
    if (self.superview != target && target && mode != 0) {
        [self cancelInput];
        [self removeFromSuperview];
        self.frame = target.bounds;
        [target addSubview:self];
    }
    if (mode != self.mode || !CGSizeEqualToSize(self.previousSize, self.bounds.size)) {
        [self cancelInput];
        self.mode = mode;
        self.previousSize = self.bounds.size;

        [self setNeedsDisplay];
    }
    [self loadSharedLayout];
    self.guiSize = CGSizeMake((status >> 8) & 0xFFFF, (status >> 24) & 0xFFFF);
    self.hidden = mode == 0;
    BOOL worldGestures = [self.layout[@"worldGestures"] boolValue];
    BOOL hasAttackButton = [self.layout[@"attackButton"] boolValue];
    // Long-press mining is a single held mouse button, never a click timer.
    for (NCLRTouchFinger *finger in self.fingers.objectEnumerator) {
        if ([finger.key isEqual:@"look"] && !finger.dragged && !finger.held &&
            worldGestures && !hasAttackButton && CACurrentMediaTime() - finger.began >= 0.35) {
            finger.label = @"MouseLeft";
            finger.held = YES;
            [self holdLabel:finger.label down:YES];
        }
    }
    [self publishVisuals];
    (void)display;
}

- (UIView *)hitTest:(CGPoint)point withEvent:(UIEvent *)event {
    // Indirect pointer events continue to UIKit/GameController's mouse path.
    if (self.hidden || !CGRectContainsPoint(self.bounds, point) || event.type != UIEventTypeTouches) return nil;
    for (UITouch *touch in event.allTouches) {
        if (touch.type == UITouchTypeDirect || touch.type == UITouchTypePencil) return self;
    }
    return nil;
}

- (NSString *)keyAt:(CGPoint)point {
    for (NSString *key in self.visibleKeys.reverseObjectEnumerator) {
        CGRect rect = [self rectForKey:key];
        if ([key isEqual:@"move"] && [self.layout[@"dynamic"] boolValue]) {
            NSArray *area = self.layout[@"dynamicArea"];
            if (area.count == 4) rect = CGRectMake([area[0] doubleValue] * self.bounds.size.width,
                [area[1] doubleValue] * self.bounds.size.height, [area[2] doubleValue] * self.bounds.size.width,
                [area[3] doubleValue] * self.bounds.size.height);
        }
        if (CGRectContainsPoint(rect, point)) return key;
    }
    return nil;
}

- (void)moveCursor:(CGPoint)point {
    CGFloat scale = self.superview.contentScaleFactor;
    horizon_ios_hardware_mouse_motion(0, 0, point.x * scale, point.y * scale);
}

- (void)updateJoystick:(NCLRTouchFinger *)finger point:(CGPoint)point {
    CGFloat radius = MAX(1, [self rectForKey:@"move"].size.height / 2);
    CGFloat x = (point.x - finger.anchor.x) / radius;
    CGFloat y = (point.y - finger.anchor.y) / radius;
    CGFloat deadzone = [NCLRKeymap.shared.touchMode isEqual:@"dpad"] ? 1.0 / 3.0 : NCLRKeymap.shared.joystickDeadzone;
    NSMutableSet *next = [NSMutableSet set];
    NSDictionary *directions = @{@"left": @(x < -deadzone), @"right": @(x > deadzone),
        @"forward": @(y < -deadzone), @"back": @(y > deadzone),
        @"sprint": @(NCLRKeymap.shared.joystickSprint && y < -1.0)};
    for (NSString *key in directions) {
        NSString *label = NCLRKeymap.shared.bindings[key];
        if ([directions[key] boolValue] && label.length) [next addObject:label];
    }
    for (NSString *label in self.moveLabels) {
        if (![next containsObject:label]) [self holdLabel:label down:NO];
    }
    for (NSString *label in next) {
        if (![self.moveLabels containsObject:label]) [self holdLabel:label down:YES];
    }
    self.moveLabels = next;
}

- (void)touchesBegan:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    for (UITouch *touch in touches) {
        if (touch.type != UITouchTypeDirect && touch.type != UITouchTypePencil) continue;
        CGPoint point = [touch locationInView:self];
        NCLRTouchFinger *finger = [NCLRTouchFinger new];
        finger.anchor = finger.previous = point;
        finger.began = CACurrentMediaTime();
        NSString *key = [self keyAt:point];
        finger.key = key ?: (self.mode == 1 ? @"look" : @"cursor");
        [self.fingers setObject:finger forKey:touch];
        if ([finger.key isEqual:@"move"]) {
            BOOL owned = NO;
            for (NCLRTouchFinger *other in self.fingers.objectEnumerator)
                if (other != finger && [other.key isEqual:@"move"]) owned = YES;
            if (owned) { finger.key = @"ignore"; continue; }
            if (![self.layout[@"dynamic"] boolValue]) {
                CGRect rect = [self rectForKey:@"move"];
                finger.anchor = CGPointMake(CGRectGetMidX(rect), CGRectGetMidY(rect));
            }
            [self updateJoystick:finger point:point];
        } else if ([finger.key isEqual:@"cursor"]) {
            [self moveCursor:point];
            finger.label = @"MouseLeft";
            finger.held = YES;
            [self holdLabel:finger.label down:YES];
        } else if ([finger.key isEqual:@"look"]) {
            // Hit-test the hotbar using the engine's published GUI dimensions.
            CGFloat gx = point.x / MAX(1, self.bounds.size.width) * self.guiSize.width;
            CGFloat gy = point.y / MAX(1, self.bounds.size.height) * self.guiSize.height;
            CGFloat left = (self.guiSize.width - 182) / 2;
            if (self.guiSize.width >= 182 && gy >= self.guiSize.height - 22 && gx >= left && gx < left + 182) {
                NSUInteger slot = MIN(8, (NSUInteger)((gx - left) / 20));
                NSString *label = [NSString stringWithFormat:@"%lu", (unsigned long)(slot + 1)];
                [self holdLabel:label down:YES];
                [self holdLabel:label down:NO];
                finger.key = @"ignore";
            }
        } else if (key) {
            if ([key isEqual:@"keyboard"] || [key isEqual:NCLRInventoryKeyboard] || [key isEqual:NCLRPauseKeyboard]) {
                horizon_ios_touch_keyboard();
                continue;
            }
            NSDictionary *button = self.buttons[key];
            finger.label = button[@"label"] ?: [NCLRKeymap.shared labelForCanvasKey:key];
            if ([@[@"Keyboard", @"键盘", @"系统键盘"] containsObject:finger.label]) {
                horizon_ios_touch_keyboard();
                continue;
            }
            finger.swipe = [button[@"swipe"] boolValue];
            if ([button[@"toggle"] boolValue]) {
                BOOL down = ![self.latchedKeys containsObject:key];
                if (down) [self.latchedKeys addObject:key]; else [self.latchedKeys removeObject:key];
                [self holdLabel:finger.label down:down];
            } else {
                finger.held = YES;
                [self holdLabel:finger.label down:YES];
            }
        }
    }
    [self publishVisuals];
    (void)event;
}

- (void)touchesMoved:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    for (UITouch *touch in touches) {
        NCLRTouchFinger *finger = [self.fingers objectForKey:touch];
        if (!finger) continue;
        CGPoint point = [touch locationInView:self];
        finger.dragged |= hypot(point.x - finger.anchor.x, point.y - finger.anchor.y) > 8;
        if ([finger.key isEqual:@"move"]) {
            [self updateJoystick:finger point:point];
        } else if ([finger.key isEqual:@"cursor"]) {
            [self moveCursor:point];
        } else if (self.mode == 1 && ([finger.key isEqual:@"look"] || finger.swipe)) {
            NSArray *sensitivity = self.layout[@"lookSensitivity"];
            CGFloat horizontal = sensitivity.count == 2 ? [sensitivity[0] doubleValue] : 1;
            CGFloat vertical = sensitivity.count == 2 ? [sensitivity[1] doubleValue] : 1;
            CGFloat unit = 1200.0 / MAX(1, MAX(self.bounds.size.width, self.bounds.size.height));
            horizon_ios_touch_look((point.x - finger.previous.x) * unit * horizontal,
                                   (point.y - finger.previous.y) * unit * vertical);
        }
        finger.previous = point;
    }
    (void)event;
}

- (void)finishTouches:(NSSet<UITouch *> *)touches cancelled:(BOOL)cancelled {
    for (UITouch *touch in touches) {
        NCLRTouchFinger *finger = [self.fingers objectForKey:touch];
        if (!finger) continue;
        if ([finger.key isEqual:@"move"]) {
            for (NSString *label in self.moveLabels) [self holdLabel:label down:NO];
            [self.moveLabels removeAllObjects];
        } else if (finger.held) {
            [self holdLabel:finger.label down:NO];
        } else if (!cancelled && [finger.key isEqual:@"look"] && !finger.dragged &&
                   [self.layout[@"worldGestures"] boolValue]) {
            [self holdLabel:@"MouseLeft" down:YES];
            [self holdLabel:@"MouseLeft" down:NO];
        }
        [self.fingers removeObjectForKey:touch];
    }
    [self publishVisuals];
}

- (void)touchesEnded:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    [self finishTouches:touches cancelled:NO];
    (void)event;
}
- (void)touchesCancelled:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    // UIKit cancellation ends toggles as well as fingers.
    [self cancelInput];
    (void)touches;
    (void)event;
}

// The transparent native view owns fingers. The original HUD draws the skin.


@end

static void NCLRTouchOverlayStart(void) { [[NCLRTouchOverlay shared] start]; }
static void NCLRTouchOverlayStop(void) { [[NCLRTouchOverlay shared] stop]; }

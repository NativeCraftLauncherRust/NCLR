#import <UIKit/UIKit.h>
#import <Security/Security.h>
#import <math.h>
#import <stdlib.h>

// The Rust side of the launcher: route3's device ABI (download, assemble, auth)
// and the engine entry point. These symbols come from libhorizon_ios.a.
extern const char *horizon_ios_check_resources(const char *resources);
extern const char *horizon_ios_assemble(const char *bundle, const char *client_jar,
                                        const char *index, const char *store,
                                        const char *out);
extern const char *horizon_ios_fetch(const char *version, const char *jar_dir,
                                     const char *store_dir);
extern const char *horizon_ios_auth_start(const char *client_id);
extern const char *horizon_ios_auth_poll(const char *client_id, const char *device_code);
extern const char *horizon_ios_auth_minecraft(const char *microsoft_token);
extern const char *horizon_ios_forget_downloads(const char *jar_dir, const char *store_dir);
extern int horizon_ios_run(const char *resources, const char *game_dir);

static NSString *const kNCLRService = @"dev.zrxmaxlnx.nclr";
static NSString *const kNCLRAccount = @"minecraft-access-token";
static NSString *const kNCLRLaunchKey = @"NCLRLaunchEngine";
static NSString *const kNCLRClientKey = @"NCLRMicrosoftClientID";
static NSString *const kNCLRResourceConcurrencyKey = @"NCLRResourceDownloadConcurrency";

static NSInteger NCLRResourceDownloadConcurrency(void) {
    NSInteger value = [[NSUserDefaults standardUserDefaults] integerForKey:kNCLRResourceConcurrencyKey];
    return (value >= 1 && value <= 8) ? value : 4;
}

// Set in main() before UIApplicationMain. The scene delegate must create the
// launcher window on the first phase, but leave window creation to winit on the
// engine phase. A UIWindow not attached to a UIWindowScene never appears on
// iOS 13+, which is a black screen with no crash report.
static BOOL NCLREngineLaunch = NO;

static NSString *NCLRString(const char *text) {
    return text == NULL ? nil : [NSString stringWithUTF8String:text];
}

static NSString *NCLRDocuments(void) {
    NSArray<NSString *> *paths =
        NSSearchPathForDirectoriesInDomains(NSDocumentDirectory, NSUserDomainMask, YES);
    return paths.firstObject ?: NSTemporaryDirectory();
}

static NSString *NCLRGameDirectory(void) {
    NSString *dir = NCLRDocuments();
    [[NSFileManager defaultManager] createDirectoryAtPath:dir
                              withIntermediateDirectories:YES
                                               attributes:nil
                                                    error:nil];
    return dir;
}

static NSString *NCLRResourcesDirectory(void) {
    return [NCLRGameDirectory() stringByAppendingPathComponent:@"horizon-client/26.3"];
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
    OSStatus status = SecItemCopyMatching((__bridge CFDictionaryRef)query, &result);
    if (status != errSecSuccess || result == NULL) {
        return nil;
    }
    NSData *data = (__bridge_transfer NSData *)result;
    return [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
}

static void NCLRKeychainWrite(NSString *value) {
    if (value.length == 0) {
        NSDictionary *query = @{
            (__bridge id)kSecClass : (__bridge id)kSecClassGenericPassword,
            (__bridge id)kSecAttrService : kNCLRService,
            (__bridge id)kSecAttrAccount : kNCLRAccount,
        };
        SecItemDelete((__bridge CFDictionaryRef)query);
        return;
    }
    NSData *data = [value dataUsingEncoding:NSUTF8StringEncoding];
    NSDictionary *query = @{
        (__bridge id)kSecClass : (__bridge id)kSecClassGenericPassword,
        (__bridge id)kSecAttrService : kNCLRService,
        (__bridge id)kSecAttrAccount : kNCLRAccount,
    };
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

static UIColor *NCLRColor(CGFloat r, CGFloat g, CGFloat b, CGFloat a) {
    return [UIColor colorWithRed:r green:g blue:b alpha:a];
}

@interface NCLRLauncherController : UIViewController
@property (nonatomic, copy) NSString *clientId;
@property (nonatomic, copy) NSString *mcToken;
@property (nonatomic, copy) NSString *deviceCode;
@property (nonatomic, copy) NSString *userCode;
@property (nonatomic, assign) NSInteger pollInterval;
@property (nonatomic, strong) NSTimer *pollTimer;
@property (nonatomic, assign) BOOL busy;
@property (nonatomic, assign) BOOL resourcesReady;
@property (nonatomic, strong) UILabel *accountStatus;
@property (nonatomic, strong) UILabel *accountDetail;
@property (nonatomic, strong) UILabel *resourceStatus;
@property (nonatomic, strong) UILabel *logLabel;
@property (nonatomic, strong) UIProgressView *progress;
@property (nonatomic, strong) UIActivityIndicatorView *spinner;
@property (nonatomic, strong) UIButton *loginButton;
@property (nonatomic, strong) UIButton *prepareButton;
@property (nonatomic, strong) UIButton *playButton;
@end

@implementation NCLRLauncherController

- (void)viewDidLoad {
    [super viewDidLoad];
    self.clientId = [[NSUserDefaults standardUserDefaults] stringForKey:kNCLRClientKey] ?: @"";
    self.mcToken = NCLRKeychainRead();
    self.pollInterval = 5;
    [self buildInterface];
    [self refreshInterface];
    [self checkResources];
}

- (void)dealloc {
    [self.pollTimer invalidate];
}

- (void)viewDidLayoutSubviews {
    [super viewDidLayoutSubviews];
    for (CALayer *layer in self.view.layer.sublayers) {
        if ([layer isKindOfClass:[CAGradientLayer class]]) {
            layer.frame = self.view.bounds;
        }
    }
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

- (UIView *)card {
    UIView *card = [[UIView alloc] init];
    card.translatesAutoresizingMaskIntoConstraints = NO;
    card.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.055];
    card.layer.cornerRadius = 18.0;
    card.layer.borderWidth = 1.0;
    card.layer.borderColor = [UIColor colorWithWhite:1.0 alpha:0.08].CGColor;
    return card;
}

- (UIButton *)button:(NSString *)title action:(SEL)action primary:(BOOL)primary {
    UIButton *button = [UIButton buttonWithType:UIButtonTypeSystem];
    [button setTitle:title forState:UIControlStateNormal];
    button.titleLabel.font = [UIFont systemFontOfSize:15.0 weight:UIFontWeightSemibold];
    button.layer.cornerRadius = 12.0;
    button.translatesAutoresizingMaskIntoConstraints = NO;
    [button.heightAnchor constraintEqualToConstant:46.0].active = YES;
    if (primary) {
        [button setTitleColor:NCLRColor(0.03, 0.16, 0.10, 1.0) forState:UIControlStateNormal];
        button.backgroundColor = NCLRColor(0.55, 1.0, 0.78, 1.0);
    } else {
        [button setTitleColor:NCLRColor(0.82, 0.88, 0.94, 1.0) forState:UIControlStateNormal];
        button.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.075];
        button.layer.borderWidth = 1.0;
        button.layer.borderColor = [UIColor colorWithWhite:1.0 alpha:0.12].CGColor;
    }
    [button addTarget:self action:action forControlEvents:UIControlEventTouchUpInside];
    return button;
}

- (void)buildInterface {
    self.view.backgroundColor = NCLRColor(0.027, 0.043, 0.065, 1.0);
    CAGradientLayer *gradient = [CAGradientLayer layer];
    gradient.colors = @[
        (__bridge id)NCLRColor(0.04, 0.10, 0.11, 1.0).CGColor,
        (__bridge id)NCLRColor(0.027, 0.043, 0.065, 1.0).CGColor,
        (__bridge id)NCLRColor(0.06, 0.04, 0.10, 1.0).CGColor,
    ];
    gradient.startPoint = CGPointMake(0.1, 0.0);
    gradient.endPoint = CGPointMake(0.9, 1.0);
    [self.view.layer insertSublayer:gradient atIndex:0];

    UIScrollView *scroll = [[UIScrollView alloc] init];
    scroll.translatesAutoresizingMaskIntoConstraints = NO;
    scroll.alwaysBounceVertical = YES;
    scroll.showsVerticalScrollIndicator = NO;
    [self.view addSubview:scroll];

    UIStackView *stack = [[UIStackView alloc] init];
    stack.axis = UILayoutConstraintAxisVertical;
    stack.spacing = 14.0;
    stack.translatesAutoresizingMaskIntoConstraints = NO;
    [scroll addSubview:stack];

    [NSLayoutConstraint activateConstraints:@[
        [scroll.leadingAnchor constraintEqualToAnchor:self.view.leadingAnchor],
        [scroll.trailingAnchor constraintEqualToAnchor:self.view.trailingAnchor],
        [scroll.topAnchor constraintEqualToAnchor:self.view.topAnchor],
        [scroll.bottomAnchor constraintEqualToAnchor:self.view.bottomAnchor],
        [stack.leadingAnchor constraintEqualToAnchor:scroll.frameLayoutGuide.leadingAnchor constant:22.0],
        [stack.trailingAnchor constraintEqualToAnchor:scroll.frameLayoutGuide.trailingAnchor constant:-22.0],
        [stack.topAnchor constraintEqualToAnchor:scroll.contentLayoutGuide.topAnchor constant:26.0],
        [stack.bottomAnchor constraintEqualToAnchor:scroll.contentLayoutGuide.bottomAnchor constant:-24.0],
    ]];

    // Header
    UIView *header = [[UIView alloc] init];
    header.translatesAutoresizingMaskIntoConstraints = NO;
    UILabel *mark = [self label:@"N" size:30.0 weight:UIFontWeightHeavy color:NCLRColor(0.55, 1.0, 0.78, 1.0)];
    mark.textAlignment = NSTextAlignmentCenter;
    mark.backgroundColor = [UIColor colorWithWhite:1.0 alpha:0.055];
    mark.layer.cornerRadius = 14.0;
    mark.layer.masksToBounds = YES;
    mark.layer.borderWidth = 1.0;
    mark.layer.borderColor = [UIColor colorWithWhite:1.0 alpha:0.10].CGColor;
    mark.translatesAutoresizingMaskIntoConstraints = NO;
    [header addSubview:mark];
    UILabel *title = [self label:@"NCLR" size:32.0 weight:UIFontWeightBold color:NCLRColor(0.93, 0.96, 1.0, 1.0)];
    UILabel *subtitle = [self label:@"MINECRAFT: JAVA EDITION · RUST" size:10.0 weight:UIFontWeightSemibold color:NCLRColor(0.50, 0.62, 0.72, 1.0)];
    subtitle.attributedText = [[NSAttributedString alloc] initWithString:subtitle.text attributes:@{
        NSKernAttributeName : @1.4
    }];
    [header addSubview:title];
    [header addSubview:subtitle];
    UIButton *settings = [self button:@"设置" action:@selector(settingsTapped) primary:NO];
    [header addSubview:settings];
    [NSLayoutConstraint activateConstraints:@[
        [mark.leadingAnchor constraintEqualToAnchor:header.leadingAnchor],
        [mark.topAnchor constraintEqualToAnchor:header.topAnchor],
        [mark.widthAnchor constraintEqualToConstant:58.0],
        [mark.heightAnchor constraintEqualToConstant:58.0],
        [title.leadingAnchor constraintEqualToAnchor:mark.trailingAnchor constant:14.0],
        [title.topAnchor constraintEqualToAnchor:mark.topAnchor constant:2.0],
        [subtitle.leadingAnchor constraintEqualToAnchor:title.leadingAnchor],
        [subtitle.topAnchor constraintEqualToAnchor:title.bottomAnchor constant:4.0],
        [settings.trailingAnchor constraintEqualToAnchor:header.trailingAnchor],
        [settings.centerYAnchor constraintEqualToAnchor:mark.centerYAnchor],
        [settings.widthAnchor constraintEqualToConstant:70.0],
        [settings.heightAnchor constraintEqualToConstant:34.0],
        [header.heightAnchor constraintGreaterThanOrEqualToConstant:58.0],
        [header.bottomAnchor constraintGreaterThanOrEqualToAnchor:mark.bottomAnchor],
        [header.bottomAnchor constraintGreaterThanOrEqualToAnchor:subtitle.bottomAnchor],
    ]];
    [stack addArrangedSubview:header];

    // Account
    UIView *account = [self card];
    UIStackView *accountStack = [[UIStackView alloc] init];
    accountStack.axis = UILayoutConstraintAxisVertical;
    accountStack.spacing = 8.0;
    accountStack.translatesAutoresizingMaskIntoConstraints = NO;
    [account addSubview:accountStack];
    [NSLayoutConstraint activateConstraints:@[
        [accountStack.leadingAnchor constraintEqualToAnchor:account.leadingAnchor constant:16.0],
        [accountStack.trailingAnchor constraintEqualToAnchor:account.trailingAnchor constant:-16.0],
        [accountStack.topAnchor constraintEqualToAnchor:account.topAnchor constant:14.0],
        [accountStack.bottomAnchor constraintEqualToAnchor:account.bottomAnchor constant:-14.0],
    ]];
    UILabel *accountEyebrow = [self label:@"MICROSOFT 正版" size:9.0 weight:UIFontWeightBold color:NCLRColor(0.48, 0.58, 0.68, 1.0)];
    accountEyebrow.attributedText = [[NSAttributedString alloc] initWithString:accountEyebrow.text attributes:@{
        NSKernAttributeName : @1.6
    }];
    self.accountStatus = [self label:@"未登录" size:19.0 weight:UIFontWeightSemibold color:NCLRColor(0.92, 0.96, 1.0, 1.0)];
    self.accountDetail = [self label:@"" size:11.0 weight:UIFontWeightRegular color:NCLRColor(0.55, 0.64, 0.73, 1.0)];
    self.loginButton = [self button:@"使用 Microsoft 登录" action:@selector(loginTapped) primary:YES];
    [accountStack addArrangedSubview:accountEyebrow];
    [accountStack addArrangedSubview:self.accountStatus];
    [accountStack addArrangedSubview:self.accountDetail];
    [accountStack addArrangedSubview:self.loginButton];
    [stack addArrangedSubview:account];

    // Resources
    UIView *resources = [self card];
    UIStackView *resourceStack = [[UIStackView alloc] init];
    resourceStack.axis = UILayoutConstraintAxisVertical;
    resourceStack.spacing = 8.0;
    resourceStack.translatesAutoresizingMaskIntoConstraints = NO;
    [resources addSubview:resourceStack];
    [NSLayoutConstraint activateConstraints:@[
        [resourceStack.leadingAnchor constraintEqualToAnchor:resources.leadingAnchor constant:16.0],
        [resourceStack.trailingAnchor constraintEqualToAnchor:resources.trailingAnchor constant:-16.0],
        [resourceStack.topAnchor constraintEqualToAnchor:resources.topAnchor constant:14.0],
        [resourceStack.bottomAnchor constraintEqualToAnchor:resources.bottomAnchor constant:-14.0],
    ]];
    UILabel *resourceEyebrow = [self label:@"GAME RESOURCES" size:9.0 weight:UIFontWeightBold color:NCLRColor(0.48, 0.58, 0.68, 1.0)];
    resourceEyebrow.attributedText = [[NSAttributedString alloc] initWithString:resourceEyebrow.text attributes:@{
        NSKernAttributeName : @1.6
    }];
    self.resourceStatus = [self label:@"正在检查 26.3…" size:19.0 weight:UIFontWeightSemibold color:NCLRColor(0.92, 0.96, 1.0, 1.0)];
    UILabel *resourceDetail = [self label:@"下载只走 Mojang 官方端点；装配在设备上完成，不把原始 jar/assets 打进 IPA。" size:11.0 weight:UIFontWeightRegular color:NCLRColor(0.55, 0.64, 0.73, 1.0)];
    self.progress = [[UIProgressView alloc] initWithProgressViewStyle:UIProgressViewStyleDefault];
    self.progress.translatesAutoresizingMaskIntoConstraints = NO;
    self.progress.progressTintColor = NCLRColor(0.55, 1.0, 0.78, 1.0);
    self.progress.trackTintColor = [UIColor colorWithWhite:1.0 alpha:0.10];
    self.progress.progress = 0.0;
    self.prepareButton = [self button:@"下载并准备 26.3" action:@selector(prepareTapped) primary:NO];
    [resourceStack addArrangedSubview:resourceEyebrow];
    [resourceStack addArrangedSubview:self.resourceStatus];
    [resourceStack addArrangedSubview:resourceDetail];
    [resourceStack addArrangedSubview:self.progress];
    [resourceStack addArrangedSubview:self.prepareButton];
    [stack addArrangedSubview:resources];

    // Play
    UIView *playCard = [self card];
    UIStackView *playStack = [[UIStackView alloc] init];
    playStack.axis = UILayoutConstraintAxisVertical;
    playStack.spacing = 10.0;
    playStack.translatesAutoresizingMaskIntoConstraints = NO;
    [playCard addSubview:playStack];
    [NSLayoutConstraint activateConstraints:@[
        [playStack.leadingAnchor constraintEqualToAnchor:playCard.leadingAnchor constant:16.0],
        [playStack.trailingAnchor constraintEqualToAnchor:playCard.trailingAnchor constant:-16.0],
        [playStack.topAnchor constraintEqualToAnchor:playCard.topAnchor constant:14.0],
        [playStack.bottomAnchor constraintEqualToAnchor:playCard.bottomAnchor constant:-14.0],
    ]];
    self.playButton = [self button:@"开始游戏" action:@selector(playTapped) primary:YES];
    self.playButton.titleLabel.font = [UIFont systemFontOfSize:18.0 weight:UIFontWeightBold];
    [self.playButton.heightAnchor constraintEqualToConstant:54.0].active = YES;
    UILabel *playHint = [self label:@"资源准备完成后，NCLR 会退出一次；从主屏幕重新打开即可进入游戏。这是 iOS 单进程生命周期限制，不是崩溃。" size:10.0 weight:UIFontWeightRegular color:NCLRColor(0.48, 0.57, 0.66, 1.0)];
    [playStack addArrangedSubview:self.playButton];
    [playStack addArrangedSubview:playHint];
    [stack addArrangedSubview:playCard];

    // Log + footer
    self.spinner = [[UIActivityIndicatorView alloc] initWithActivityIndicatorStyle:UIActivityIndicatorViewStyleMedium];
    self.spinner.color = NCLRColor(0.55, 1.0, 0.78, 1.0);
    self.spinner.hidesWhenStopped = YES;
    [stack addArrangedSubview:self.spinner];

    self.logLabel = [self label:@"" size:10.0 weight:UIFontWeightRegular color:NCLRColor(0.48, 0.57, 0.66, 1.0)];
    self.logLabel.font = [UIFont monospacedSystemFontOfSize:10.0 weight:UIFontWeightRegular];
    [stack addArrangedSubview:self.logLabel];

    UILabel *footer = [self label:@"作者 ZRXMAXLNX 与 可心 · NCLR 0.1.0\n不含 Mojang 原始文件 · 需要正版账号 · 非官方产品" size:9.0 weight:UIFontWeightRegular color:NCLRColor(0.33, 0.40, 0.48, 1.0)];
    footer.textAlignment = NSTextAlignmentCenter;
    [stack addArrangedSubview:footer];
}

- (void)refreshInterface {
    BOOL loggedIn = self.mcToken.length > 0;
    self.accountStatus.text = loggedIn ? @"已登录 Microsoft" : @"未登录";
    self.accountDetail.text = self.clientId.length > 0
        ? [NSString stringWithFormat:@"Azure Client ID 已配置 · %@", [self.clientId substringToIndex:MIN((NSUInteger)8, self.clientId.length)]]
        : @"需要配置自己的 Azure Client ID 才能登录";
    [self.loginButton setTitle:(loggedIn ? @"切换 / 重新登录" : @"使用 Microsoft 登录") forState:UIControlStateNormal];
    self.loginButton.enabled = !self.busy;

    self.resourceStatus.text = self.resourcesReady ? @"26.3 已准备" : @"26.3 未准备";
    [self.prepareButton setTitle:(self.resourcesReady ? @"重新检查 / 重新准备" : @"下载并准备 26.3") forState:UIControlStateNormal];
    self.prepareButton.enabled = !self.busy && loggedIn;
    self.playButton.enabled = self.resourcesReady && !self.busy;
    self.playButton.alpha = self.playButton.enabled ? 1.0 : 0.45;
    self.loginButton.alpha = self.loginButton.enabled ? 1.0 : 0.45;
    self.prepareButton.alpha = self.prepareButton.enabled ? 1.0 : 0.45;
    if (self.busy) {
        [self.spinner startAnimating];
    } else {
        [self.spinner stopAnimating];
    }
}

- (void)setBusy:(BOOL)busy log:(NSString *)log {
    self.busy = busy;
    if (log.length > 0) {
        self.logLabel.text = log;
    }
    [self refreshInterface];
}

- (void)showAlert:(NSString *)title message:(NSString *)message {
    UIAlertController *alert = [UIAlertController alertControllerWithTitle:title
                                                                   message:message
                                                            preferredStyle:UIAlertControllerStyleAlert];
    [alert addAction:[UIAlertAction actionWithTitle:@"好" style:UIAlertActionStyleDefault handler:nil]];
    [self presentViewController:alert animated:YES completion:nil];
}

- (void)checkResources {
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSString *resources = NCLRResourcesDirectory();
        const char *failure = horizon_ios_check_resources(resources.UTF8String);
        NSString *message = NCLRString(failure);
        dispatch_async(dispatch_get_main_queue(), ^{
            self.resourcesReady = (message == nil);
            self.resourceStatus.text = self.resourcesReady ? @"26.3 已准备" : @"26.3 未准备";
            self.logLabel.text = self.resourcesReady ? @"资源检查通过。" : (message ?: @"资源未准备。");
            [self refreshInterface];
        });
    });
}

- (void)promptForClientID {
    UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"Microsoft Client ID"
                                                                   message:@"填入你自己的 Azure Application (client) ID。不要复用其他启动器的 ID。"
                                                            preferredStyle:UIAlertControllerStyleAlert];
    [alert addTextFieldWithConfigurationHandler:^(UITextField *textField) {
        textField.placeholder = @"00000000-0000-0000-0000-000000000000";
        textField.text = self.clientId;
        textField.autocapitalizationType = UITextAutocapitalizationTypeNone;
        textField.autocorrectionType = UITextAutocorrectionTypeNo;
    }];
    [alert addAction:[UIAlertAction actionWithTitle:@"取消" style:UIAlertActionStyleCancel handler:nil]];
    [alert addAction:[UIAlertAction actionWithTitle:@"保存" style:UIAlertActionStyleDefault handler:^(UIAlertAction *action) {
        NSString *value = [alert.textFields.firstObject.text stringByTrimmingCharactersInSet:
                           [NSCharacterSet whitespaceAndNewlineCharacterSet]];
        self.clientId = value ?: @"";
        [[NSUserDefaults standardUserDefaults] setObject:self.clientId forKey:kNCLRClientKey];
        [self refreshInterface];
    }]];
    [self presentViewController:alert animated:YES completion:nil];
}

- (void)settingsTapped {
    UIViewController *panel = [[UIViewController alloc] init];
    panel.view.backgroundColor = NCLRColor(0.027, 0.043, 0.065, 1.0);
    panel.modalPresentationStyle = UIModalPresentationFormSheet;
    panel.preferredContentSize = CGSizeMake(420.0, 520.0);

    UIStackView *stack = [[UIStackView alloc] init];
    stack.axis = UILayoutConstraintAxisVertical;
    stack.spacing = 14.0;
    stack.translatesAutoresizingMaskIntoConstraints = NO;
    [panel.view addSubview:stack];
    [NSLayoutConstraint activateConstraints:@[
        [stack.leadingAnchor constraintEqualToAnchor:panel.view.safeAreaLayoutGuide.leadingAnchor constant:22.0],
        [stack.trailingAnchor constraintEqualToAnchor:panel.view.safeAreaLayoutGuide.trailingAnchor constant:-22.0],
        [stack.topAnchor constraintEqualToAnchor:panel.view.safeAreaLayoutGuide.topAnchor constant:22.0],
        [stack.bottomAnchor constraintEqualToAnchor:panel.view.safeAreaLayoutGuide.bottomAnchor constant:-22.0],
    ]];

    UILabel *title = [self label:@"NCLR 设置" size:25.0 weight:UIFontWeightBold color:NCLRColor(0.93, 0.96, 1.0, 1.0)];
    UILabel *subtitle = [self label:@"登录应用 ID 只保存在本机。" size:12.0 weight:UIFontWeightRegular color:NCLRColor(0.55, 0.64, 0.73, 1.0)];
    [stack addArrangedSubview:title];
    [stack addArrangedSubview:subtitle];

    UILabel *concurrencyTitle = [self label:@"资源下载并发限制" size:16.0 weight:UIFontWeightSemibold color:NCLRColor(0.92, 0.96, 1.0, 1.0)];
    NSInteger savedConcurrency = NCLRResourceDownloadConcurrency();
    UILabel *concurrencyValue = [self label:[NSString stringWithFormat:@"%ld", (long)savedConcurrency]
                                       size:18.0
                                     weight:UIFontWeightBold
                                      color:NCLRColor(0.55, 1.0, 0.78, 1.0)];
    concurrencyValue.textAlignment = NSTextAlignmentRight;
    concurrencyValue.accessibilityLabel = @"当前资源下载并发数";
    UIStackView *concurrencyHeader = [[UIStackView alloc] init];
    concurrencyHeader.axis = UILayoutConstraintAxisHorizontal;
    concurrencyHeader.alignment = UIStackViewAlignmentCenter;
    concurrencyHeader.spacing = 8.0;
    [concurrencyHeader addArrangedSubview:concurrencyTitle];
    [concurrencyHeader addArrangedSubview:concurrencyValue];
    [concurrencyTitle setContentHuggingPriority:UILayoutPriorityDefaultLow forAxis:UILayoutConstraintAxisHorizontal];
    [concurrencyValue setContentHuggingPriority:UILayoutPriorityRequired forAxis:UILayoutConstraintAxisHorizontal];

    UISlider *slider = [[UISlider alloc] init];
    slider.minimumValue = 1.0;
    slider.maximumValue = 8.0;
    slider.minimumTrackTintColor = NCLRColor(0.55, 1.0, 0.78, 1.0);
    slider.maximumTrackTintColor = [UIColor colorWithWhite:1.0 alpha:0.18];
    slider.value = (float)savedConcurrency;
    slider.continuous = YES;
    slider.translatesAutoresizingMaskIntoConstraints = NO;
    [slider.heightAnchor constraintGreaterThanOrEqualToConstant:44.0].active = YES;
    slider.accessibilityLabel = @"资源下载并发限制";
    slider.accessibilityValue = [NSString stringWithFormat:@"%ld", (long)savedConcurrency];
    __weak UISlider *weakSlider = slider;
    __weak UILabel *weakConcurrencyValue = concurrencyValue;
    [slider addAction:[UIAction actionWithHandler:^(UIAction *action) {
        UISlider *activeSlider = weakSlider;
        UILabel *activeValue = weakConcurrencyValue;
        NSInteger value = (NSInteger)lroundf(activeSlider.value);
        activeSlider.value = (float)value;
        activeValue.text = [NSString stringWithFormat:@"%ld", (long)value];
        activeSlider.accessibilityValue = [NSString stringWithFormat:@"%ld", (long)value];
    }] forControlEvents:UIControlEventValueChanged];

    UILabel *rangeHint = [self label:@"1–8 个下载任务，默认 4。数值越高越容易占满带宽。" size:11.0 weight:UIFontWeightRegular color:NCLRColor(0.48, 0.57, 0.66, 1.0)];
    rangeHint.numberOfLines = 0;
    [stack addArrangedSubview:concurrencyHeader];
    [stack addArrangedSubview:slider];
    [stack addArrangedSubview:rangeHint];

    UIButton *configureButton = [self button:@"配置 Client ID" action:@selector(promptForClientID) primary:NO];
    UIButton *saveButton = [self button:@"保存并发设置" action:@selector(settingsTapped) primary:YES];
    UIButton *logoutButton = [self button:@"退出登录" action:@selector(settingsTapped) primary:NO];
    UIButton *cancelButton = [self button:@"取消" action:@selector(settingsTapped) primary:NO];
    __weak UIViewController *weakPanel = panel;
    [configureButton removeTarget:self action:@selector(promptForClientID) forControlEvents:UIControlEventTouchUpInside];
    [configureButton addAction:[UIAction actionWithHandler:^(UIAction *action) {
        [weakPanel dismissViewControllerAnimated:YES completion:^{
            [self promptForClientID];
        }];
    }] forControlEvents:UIControlEventTouchUpInside];
    [saveButton removeTarget:self action:@selector(settingsTapped) forControlEvents:UIControlEventTouchUpInside];
    [saveButton addAction:[UIAction actionWithHandler:^(UIAction *action) {
        NSInteger value = (NSInteger)lroundf(slider.value);
        [[NSUserDefaults standardUserDefaults] setInteger:value forKey:kNCLRResourceConcurrencyKey];
        [[NSUserDefaults standardUserDefaults] synchronize];
        [weakPanel dismissViewControllerAnimated:YES completion:nil];
    }] forControlEvents:UIControlEventTouchUpInside];
    [logoutButton removeTarget:self action:@selector(settingsTapped) forControlEvents:UIControlEventTouchUpInside];
    [logoutButton addAction:[UIAction actionWithHandler:^(UIAction *action) {
        self.mcToken = nil;
        NCLRKeychainWrite(nil);
        [weakPanel dismissViewControllerAnimated:YES completion:^{
            [self refreshInterface];
        }];
    }] forControlEvents:UIControlEventTouchUpInside];
    [cancelButton removeTarget:self action:@selector(settingsTapped) forControlEvents:UIControlEventTouchUpInside];
    [cancelButton addAction:[UIAction actionWithHandler:^(UIAction *action) {
        [weakPanel dismissViewControllerAnimated:YES completion:nil];
    }] forControlEvents:UIControlEventTouchUpInside];
    [stack addArrangedSubview:configureButton];
    [stack addArrangedSubview:saveButton];
    [stack addArrangedSubview:logoutButton];
    [stack addArrangedSubview:cancelButton];
    [self presentViewController:panel animated:YES completion:nil];
}

- (void)loginTapped {
    if (self.busy) {
        return;
    }
    if (self.clientId.length == 0) {
        [self promptForClientID];
        return;
    }
    [self setBusy:YES log:@"正在向 Microsoft 申请设备码…"];
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        const char *answer = horizon_ios_auth_start(self.clientId.UTF8String);
        NSString *text = NCLRString(answer);
        dispatch_async(dispatch_get_main_queue(), ^{
            [self handleAuthStart:text];
        });
    });
}

- (void)handleAuthStart:(NSString *)text {
    NSArray<NSString *> *lines = [text componentsSeparatedByString:@"\n"];
    if (lines.count < 5 || ![lines[0] isEqualToString:@"ok"]) {
        [self setBusy:NO log:nil];
        [self showAlert:@"登录失败" message:(lines.count > 1 ? lines[1] : @"无法开始 Microsoft 登录")];
        return;
    }
    self.deviceCode = lines[1];
    self.userCode = lines[2];
    NSString *uri = lines[3];
    self.pollInterval = MAX(3, lines[4].integerValue);
    [self setBusy:NO log:[NSString stringWithFormat:@"在浏览器输入代码 %@", self.userCode]];

    UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"在浏览器完成登录"
                                                                   message:[NSString stringWithFormat:@"打开 Microsoft 登录页，输入代码：\n\n%@\n\n登录完成后回到 NCLR。", self.userCode]
                                                            preferredStyle:UIAlertControllerStyleAlert];
    [alert addAction:[UIAlertAction actionWithTitle:@"复制代码" style:UIAlertActionStyleDefault handler:^(UIAlertAction *action) {
        [UIPasteboard generalPasteboard].string = self.userCode;
    }]];
    [alert addAction:[UIAlertAction actionWithTitle:@"打开浏览器" style:UIAlertActionStyleDefault handler:^(UIAlertAction *action) {
        NSURL *url = [NSURL URLWithString:uri];
        if (url != nil) {
            [[UIApplication sharedApplication] openURL:url options:@{} completionHandler:nil];
        }
    }]];
    [alert addAction:[UIAlertAction actionWithTitle:@"开始等待" style:UIAlertActionStyleDefault handler:^(UIAlertAction *action) {
        [self startPolling];
    }]];
    [self presentViewController:alert animated:YES completion:nil];
}

- (void)startPolling {
    [self.pollTimer invalidate];
    self.pollTimer = [NSTimer timerWithTimeInterval:self.pollInterval target:self selector:@selector(pollTapped) userInfo:nil repeats:YES];
    [[NSRunLoop mainRunLoop] addTimer:self.pollTimer forMode:NSRunLoopCommonModes];
    [self pollTapped];
}

- (void)pollTapped {
    if (self.deviceCode.length == 0) {
        [self.pollTimer invalidate];
        return;
    }
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        const char *answer = horizon_ios_auth_poll(self.clientId.UTF8String, self.deviceCode.UTF8String);
        NSString *text = NCLRString(answer);
        dispatch_async(dispatch_get_main_queue(), ^{
            [self handlePoll:text];
        });
    });
}

- (void)handlePoll:(NSString *)text {
    NSArray<NSString *> *lines = [text componentsSeparatedByString:@"\n"];
    NSString *status = lines.firstObject ?: @"error";
    if ([status isEqualToString:@"pending"]) {
        self.logLabel.text = @"等待 Microsoft 确认…";
        return;
    }
    if ([status isEqualToString:@"slow_down"]) {
        self.pollInterval += 5;
        [self.pollTimer invalidate];
        [self startPolling];
        return;
    }
    if ([status isEqualToString:@"token"] && lines.count > 1) {
        [self.pollTimer invalidate];
        self.logLabel.text = @"Microsoft token 已收到，正在换取 Minecraft token…";
        NSString *microsoftToken = lines[1];
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
            const char *minecraft = horizon_ios_auth_minecraft(microsoftToken.UTF8String);
            NSString *answer = NCLRString(minecraft);
            dispatch_async(dispatch_get_main_queue(), ^{
                [self handleMinecraftAuth:answer];
            });
        });
        return;
    }
    [self.pollTimer invalidate];
    self.deviceCode = nil;
    self.logLabel.text = @"登录没有完成。";
    [self showAlert:@"登录失败" message:(lines.count > 1 ? lines[1] : status)];
}

- (void)handleMinecraftAuth:(NSString *)text {
    NSArray<NSString *> *lines = [text componentsSeparatedByString:@"\n"];
    if (lines.count > 1 && [lines[0] isEqualToString:@"token"]) {
        self.mcToken = lines[1];
        NCLRKeychainWrite(self.mcToken);
        self.logLabel.text = @"已登录。接下来可以下载并准备 26.3。";
        [self refreshInterface];
        return;
    }
    self.logLabel.text = @"Minecraft 登录失败。";
    [self showAlert:@"登录失败" message:(lines.count > 1 ? lines[1] : @"无法换取 Minecraft token")];
}

- (void)prepareTapped {
    if (self.busy) {
        return;
    }
    if (self.mcToken.length == 0) {
        [self showAlert:@"需要正版账号" message:@"下载前先用拥有 Minecraft: Java Edition 的 Microsoft 账号登录。"];
        return;
    }
    NSString *ship = [[NSBundle mainBundle] pathForResource:@"ship" ofType:@"hzr3"];
    if (ship.length == 0) {
        [self showAlert:@"缺少首跑包" message:@"这个 IPA 没有 ship.hzr3，无法在设备上装配资源。"];
        return;
    }
    [self setBusy:YES log:@"正在从 Mojang 官方端点下载 26.3…"];
    self.progress.progress = 0.05;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        setenv("HORIZON_ACCESS_TOKEN", self.mcToken.UTF8String, 1);
        NSString *game = NCLRGameDirectory();
        NSString *resources = NCLRResourcesDirectory();
        NSString *jarDir = [game stringByAppendingPathComponent:@"mc"];
        NSString *assetsDir = [game stringByAppendingPathComponent:@"assets"];
        NSString *jar = [jarDir stringByAppendingPathComponent:@"client.jar"];
        NSString *index = [assetsDir stringByAppendingPathComponent:@"indexes/34.json"];
        NSString *store = [assetsDir stringByAppendingPathComponent:@"objects"];

        NSString *concurrency = [NSString stringWithFormat:@"%ld", (long)NCLRResourceDownloadConcurrency()];
        setenv("NCLR_RESOURCE_DOWNLOAD_CONCURRENCY", concurrency.UTF8String, 1);
        const char *fetchFailure = horizon_ios_fetch("26.3", jarDir.UTF8String, assetsDir.UTF8String);
        NSString *fetchMessage = NCLRString(fetchFailure);
        if (fetchMessage != nil) {
            dispatch_async(dispatch_get_main_queue(), ^{
                [self finishPrepareWithError:[NSString stringWithFormat:@"下载失败：%@", fetchMessage]];
            });
            return;
        }
        dispatch_async(dispatch_get_main_queue(), ^{
            self.progress.progress = 0.55;
            self.logLabel.text = @"下载完成，正在设备上装配资源（约 250 MB）…";
        });

        const char *assembleFailure = horizon_ios_assemble(ship.UTF8String, jar.UTF8String,
                                                           index.UTF8String, store.UTF8String,
                                                           resources.UTF8String);
        NSString *assembleMessage = NCLRString(assembleFailure);
        if (assembleMessage == nil) {
            horizon_ios_forget_downloads(jarDir.UTF8String, assetsDir.UTF8String);
        }
        const char *checkFailure = horizon_ios_check_resources(resources.UTF8String);
        NSString *checkMessage = NCLRString(checkFailure);
        dispatch_async(dispatch_get_main_queue(), ^{
            if (assembleMessage != nil) {
                [self finishPrepareWithError:[NSString stringWithFormat:@"装配失败：%@", assembleMessage]];
            } else if (checkMessage != nil) {
                [self finishPrepareWithError:[NSString stringWithFormat:@"检查失败：%@", checkMessage]];
            } else {
                self.progress.progress = 1.0;
                self.resourcesReady = YES;
                [self setBusy:NO log:@"26.3 已准备。点击「开始游戏」。"];
            }
        });
    });
}

- (void)finishPrepareWithError:(NSString *)message {
    self.progress.progress = 0.0;
    [self setBusy:NO log:message];
    [self showAlert:@"资源准备失败" message:message];
}

- (void)playTapped {
    if (!self.resourcesReady) {
        [self showAlert:@"还没有准备资源" message:@"先下载并准备 26.3。"];
        return;
    }
    if (self.mcToken.length == 0) {
        [self showAlert:@"需要正版账号" message:@"先登录 Microsoft 账号。"];
        return;
    }
    [[NSUserDefaults standardUserDefaults] setBool:YES forKey:kNCLRLaunchKey];
    [[NSUserDefaults standardUserDefaults] synchronize];
    UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"资源已就绪"
                                                                   message:@"iOS 的单进程生命周期要求 NCLR 重启一次才能进入 Rust 引擎。点击「退出 NCLR」，然后从主屏幕重新打开。"
                                                            preferredStyle:UIAlertControllerStyleAlert];
    [alert addAction:[UIAlertAction actionWithTitle:@"退出 NCLR" style:UIAlertActionStyleDestructive handler:^(UIAlertAction *action) {
        exit(0);
    }]];
    [alert addAction:[UIAlertAction actionWithTitle:@"稍后" style:UIAlertActionStyleCancel handler:nil]];
    [self presentViewController:alert animated:YES completion:nil];
}

@end

@interface NCLRSceneDelegate : UIResponder <UIWindowSceneDelegate>
@property (nonatomic, strong) UIWindow *window;
@end

@implementation NCLRSceneDelegate

- (void)scene:(UIScene *)scene
    willConnectToSession:(UISceneSession *)session
                 options:(UISceneConnectionOptions *)connectionOptions {
    // Engine phase: winit creates its own window and attaches it when this
    // scene connects. Creating a second window here would cover the game.
    if (NCLREngineLaunch) {
        return;
    }
    if (![scene isKindOfClass:[UIWindowScene class]]) {
        return;
    }
    UIWindowScene *windowScene = (UIWindowScene *)scene;
    self.window = [[UIWindow alloc] initWithWindowScene:windowScene];
    self.window.rootViewController = [[NCLRLauncherController alloc] init];
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
        NSUserDefaults *defaults = [NSUserDefaults standardUserDefaults];
        if ([defaults boolForKey:kNCLRLaunchKey]) {
            [defaults setBool:NO forKey:kNCLRLaunchKey];
            [defaults synchronize];
            NSString *game = NCLRGameDirectory();
            NSString *resources = NCLRResourcesDirectory();
            const char *failure = horizon_ios_check_resources(resources.UTF8String);
            if (failure == NULL) {
                NCLREngineLaunch = YES;
                NSString *token = NCLRKeychainRead();
                if (token.length > 0) {
                    setenv("HORIZON_ACCESS_TOKEN", token.UTF8String, 1);
                }
                setenv("HORIZON_HOME", game.UTF8String, 1);
                setenv("HORIZON_NO_AUDIO", "1", 1);
                setenv("HORIZON_NO_DISCORD", "1", 1);
                return horizon_ios_run(resources.UTF8String, game.UTF8String);
            }
        }
        return UIApplicationMain(argc, argv, nil, NSStringFromClass([NCLRAppDelegate class]));
    }
}

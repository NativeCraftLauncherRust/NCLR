#import <UIKit/UIKit.h>

NS_ASSUME_NONNULL_BEGIN

@interface NCLRSidebarController : UIViewController
- (instancetype)initWithCanvasColor:(UIColor *)canvasColor
                     selectionColor:(UIColor *)selectionColor
            selectedForegroundColor:(UIColor *)selectedForegroundColor
                    foregroundColor:(UIColor *)foregroundColor
                        accentColor:(UIColor *)accentColor
                   selectionHandler:(void (^)(NSInteger page))selectionHandler;
- (void)setSelectedPage:(NSInteger)page animated:(BOOL)animated;
- (void)updateAccountWithName:(NSString *)name
                       detail:(NSString *)detail
                    avatarURL:(nullable NSString *)avatarURL;
@end

NS_ASSUME_NONNULL_END

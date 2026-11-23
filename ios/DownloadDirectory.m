// UIDocumentPicker provides the security-scoped URL. Keep a bookmark in memory so
// a reconnect can reacquire access without extending the lifetime of an open scope.
#include <stdbool.h>
#import <dispatch/dispatch.h>
#import <UIKit/UIKit.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>

typedef void (*PickReply)(void *, const char *, const char *, bool);
typedef void (*OpenReply)(void *, const char *, void *, const char *);

@interface RSSHDirectoryPicker : NSObject <UIDocumentPickerDelegate, UIAdaptivePresentationControllerDelegate>
@property(nonatomic, assign) PickReply reply;
@property(nonatomic, assign) void *context;
@end

static RSSHDirectoryPicker *pendingPicker;

static BOOL isFileDirectory(NSURL *url) {
    NSNumber *directory = nil;
    return url.isFileURL && [url getResourceValue:&directory forKey:NSURLIsDirectoryKey error:nil] && directory.boolValue;
}

@implementation RSSHDirectoryPicker
- (void)finish:(NSString *)bookmark error:(NSString *)error needsDirectory:(BOOL)needsDirectory {
    if (!self.reply) return;
    self.reply(self.context, bookmark.UTF8String, error.UTF8String, needsDirectory);
    self.reply = NULL;
    pendingPicker = nil;
}
- (void)documentPicker:(UIDocumentPickerViewController *)controller didPickDocumentsAtURLs:(NSArray<NSURL *> *)urls {
    NSURL *url = urls.firstObject;
    if (![url startAccessingSecurityScopedResource]) {
        [self finish:nil error:nil needsDirectory:YES];
        return;
    }
    if (!isFileDirectory(url)) {
        [url stopAccessingSecurityScopedResource];
        [self finish:nil error:nil needsDirectory:YES];
        return;
    }
    NSError *error = nil;
    NSData *data = [url bookmarkDataWithOptions:NSURLBookmarkCreationMinimalBookmark includingResourceValuesForKeys:nil relativeToURL:nil error:&error];
    [url stopAccessingSecurityScopedResource];
    [self finish:[data base64EncodedStringWithOptions:0] error:error.localizedDescription needsDirectory:NO];
}
- (void)documentPickerWasCancelled:(UIDocumentPickerViewController *)controller { [self finish:nil error:nil needsDirectory:NO]; }
- (void)presentationControllerDidDismiss:(UIPresentationController *)controller { [self finish:nil error:nil needsDirectory:NO]; }
@end

void rssh_pick_directory(void *context, PickReply reply) {
    dispatch_async(dispatch_get_main_queue(), ^{
        UIViewController *presenter = nil;
        for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
            if (scene.activationState != UISceneActivationStateForegroundActive ||
                ![scene isKindOfClass:UIWindowScene.class]) continue;
            for (UIWindow *window in ((UIWindowScene *)scene).windows) {
                if (window.isKeyWindow) { presenter = window.rootViewController; break; }
            }
            if (presenter) break;
        }
        if (!presenter || pendingPicker) {
            reply(context, NULL, "Directory picker is unavailable", false);
            return;
        }
        while (presenter.presentedViewController) presenter = presenter.presentedViewController;
        UIDocumentPickerViewController *picker =
            [[UIDocumentPickerViewController alloc] initForOpeningContentTypes:@[UTTypeFolder] asCopy:NO];
        pendingPicker = [RSSHDirectoryPicker new];
        pendingPicker.reply = reply;
        pendingPicker.context = context;
        picker.delegate = pendingPicker;
        picker.presentationController.delegate = pendingPicker;
        [presenter presentViewController:picker animated:YES completion:nil];
    });
}

// NSFileCoordinator keeps the provider's coordinated URL valid throughout the
// Rust transfer. The semaphore is released by the Rust directory guard on Drop.
void rssh_access_directory(const char *encoded, void *context, OpenReply reply) {
    NSString *bookmark = [NSString stringWithUTF8String:encoded];
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        NSData *data = [[NSData alloc] initWithBase64EncodedString:bookmark options:0];
        NSError *error = nil;
        BOOL stale = NO;
        NSURL *url = data ? [NSURL URLByResolvingBookmarkData:data options:0 relativeToURL:nil bookmarkDataIsStale:&stale error:&error] : nil;
        if (stale || ![url startAccessingSecurityScopedResource]) {
            reply(context, NULL, NULL, NULL);
            return;
        }
        if (!isFileDirectory(url)) {
            [url stopAccessingSecurityScopedResource];
            reply(context, NULL, NULL, NULL);
            return;
        }
        NSFileCoordinator *coordinator = [[NSFileCoordinator alloc] initWithFilePresenter:nil];
        __block BOOL called = NO;
        [coordinator coordinateWritingItemAtURL:url options:NSFileCoordinatorWritingForMerging error:&error byAccessor:^(NSURL *coordinated) {
            called = YES;
            if (!isFileDirectory(coordinated)) {
                reply(context, NULL, NULL, NULL);
                return;
            }
            dispatch_semaphore_t done = dispatch_semaphore_create(0);
            reply(context, coordinated.fileSystemRepresentation, (__bridge_retained void *)done, NULL);
            dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
        }];
        [url stopAccessingSecurityScopedResource];
        if (!called) reply(context, NULL, NULL, error.localizedDescription.UTF8String ?: "Directory coordination failed");
    });
}

void rssh_release_directory(void *token) {
    dispatch_semaphore_t done = (__bridge_transfer dispatch_semaphore_t)token;
    dispatch_semaphore_signal(done);
}

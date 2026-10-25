#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>

extern void immich_clear_login(void *, void *, void (*)(void *));
static void cleared(void *context) { *(BOOL *)context = YES; }
static void waitFor(BOOL *done) {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:15];
    while (!*done && [deadline timeIntervalSinceNow] > 0) {
        [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode
                                beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
    }
    if (!*done) { fprintf(stderr, "FAIL: WebKit callback timed out\n"); exit(1); }
}
int main(void) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyProhibited];
        [NSApp finishLaunching];
        WKWebViewConfiguration *config = [WKWebViewConfiguration new];
        // This test never opens or clears the user's persistent profile.
        config.websiteDataStore = WKWebsiteDataStore.nonPersistentDataStore;
        WKWebView *view = [[WKWebView alloc] initWithFrame:NSZeroRect configuration:config];
        NSHTTPCookie *cookie = [NSHTTPCookie cookieWithProperties:@{
            NSHTTPCookieName: @"immich_test_session", NSHTTPCookieValue: @"test-session",
            NSHTTPCookieDomain: @"immich-reset.invalid", NSHTTPCookiePath: @"/",
            NSHTTPCookieExpires: [NSDate dateWithTimeIntervalSinceNow:3600]
        }];
        __block BOOL done = NO;
        [config.websiteDataStore.httpCookieStore setCookie:cookie completionHandler:^{ done = YES; }];
        waitFor(&done);
        __block NSUInteger count = 0;
        done = NO;
        [config.websiteDataStore.httpCookieStore getAllCookies:^(NSArray<NSHTTPCookie *> *cookies) { count = cookies.count; done = YES; }];
        waitFor(&done);
        if (count != 1) { fprintf(stderr, "FAIL: isolated session cookie was not stored\n"); return 1; }
        BOOL deletionFinished = NO;
        immich_clear_login((__bridge void *)view, &deletionFinished, cleared);
        waitFor(&deletionFinished);
        done = NO;
        [config.websiteDataStore.httpCookieStore getAllCookies:^(NSArray<NSHTTPCookie *> *cookies) { count = cookies.count; done = YES; }];
        waitFor(&done);
        if (count != 0) { fprintf(stderr, "FAIL: session cookie survived completed reset\n"); return 1; }
        puts("PASS: native reset completion follows deletion of isolated WebKit session cookie");
    }
    return 0;
}

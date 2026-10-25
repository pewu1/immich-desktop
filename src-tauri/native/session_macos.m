#import <WebKit/WebKit.h>

// Called on the main thread. The completion owns the Rust sender until deletion finishes.
void immich_clear_login(void *view, void *context, void (*done)(void *)) {
    WKWebView *webview = (__bridge WKWebView *)view;
    [webview stopLoading];
    WKWebsiteDataStore *store = webview.configuration.websiteDataStore;
    [store removeDataOfTypes:WKWebsiteDataStore.allWebsiteDataTypes
              modifiedSince:[NSDate dateWithTimeIntervalSince1970:0]
          completionHandler:^{ done(context); }];
}

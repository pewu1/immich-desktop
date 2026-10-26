#import <AppKit/AppKit.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#import <CoreGraphics/CoreGraphics.h>
#include "drag.h"
#include <sys/mman.h>
#include <fcntl.h>
#include <unistd.h>

@interface ImmichContentResult : NSObject
@property(strong) NSCondition *condition;
@property(strong) NSData *data;
@property BOOL finished;
@property BOOL cancelled;
@end
@implementation ImmichContentResult
@end

#ifdef IMMICH_NATIVE_TEST
static BOOL testChromiumTarget = YES;
#endif
static BOOL chromiumAtPoint(CGPoint cursor) {
#ifdef IMMICH_NATIVE_TEST
    return testChromiumTarget;
#else
    CFArrayRef info = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID);
    if (!info) return NO;
    NSArray *windows = CFBridgingRelease(info);
    for (NSDictionary *window in windows) {
        if ([window[(id)kCGWindowLayer] integerValue] != 0) continue;
        CGRect frame;
        NSDictionary *bounds = window[(id)kCGWindowBounds];
        if (![bounds isKindOfClass:[NSDictionary class]]) continue;
        if (!CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)bounds, &frame)
            || !CGRectContainsPoint(frame, cursor)) continue;
        pid_t pid = [window[(id)kCGWindowOwnerPID] intValue];
        NSString *bundle = [NSRunningApplication runningApplicationWithProcessIdentifier:pid].bundleIdentifier;
        return [bundle hasPrefix:@"com.google.Chrome"] || [bundle isEqual:@"org.chromium.Chromium"]
            || [bundle hasPrefix:@"com.brave.Browser"] || [bundle hasPrefix:@"com.microsoft.edgemac"];
    }
    return NO;
#endif
}
static BOOL isChromiumDestination(void) {
    CGEventRef event = CGEventCreate(NULL); if (!event) return NO;
    CGPoint point = CGEventGetLocation(event); CFRelease(event); return chromiumAtPoint(point);
}
static BOOL isChromiumDropPoint(NSPoint point) {
    return chromiumAtPoint(CGPointMake(point.x, CGRectGetHeight(CGDisplayBounds(CGMainDisplayID())) - point.y));
}

@interface ImmichPromise : NSObject <NSFilePromiseProviderDelegate, NSDraggingSource>
@property void *context;
@property ImmichCallbacks callbacks;
@property(copy) NSString *filename;
@property BOOL sessionEnded;
@property BOOL fulfilled;
@property BOOL cancelled;
@property NSUInteger writers;
@property NSInteger pasteboardGeneration;
@property(strong) NSPasteboard *pasteboard;
@property(weak) NSFilePromiseProvider *provider;
@property BOOL chromiumDestination;
@property BOOL browserReadFinished;
@property BOOL chromiumDrop;
- (NSData *)chromeContents;
- (void)finishDragAtPoint:(NSPoint)point operation:(NSDragOperation)operation;
@end
// NSFilePromiseProvider's delegate is weak; keep it for the full pasteboard lifetime.
@interface ImmichProvider : NSFilePromiseProvider
@property(strong) ImmichPromise *owner;
@end
@implementation ImmichProvider
- (NSArray<NSPasteboardType> *)writableTypesForPasteboard:(NSPasteboard *)pasteboard {
    NSMutableArray *types = [[super writableTypesForPasteboard:pasteboard] mutableCopy];
    for (NSString *type in @[self.fileType, NSPasteboardTypeHTML, @"com.apple.pasteboard.promised-file-content-type", @"org.chromium.content-disposition"])
        if (![types containsObject:type]) [types addObject:type];
    return types;
}
- (NSPasteboardWritingOptions)writingOptionsForType:(NSPasteboardType)type pasteboard:(NSPasteboard *)pasteboard {
    if ([type isEqual:self.fileType]) return NSPasteboardWritingPromised;
    return [super writingOptionsForType:type pasteboard:pasteboard];
}
- (id)pasteboardPropertyListForType:(NSPasteboardType)type {
    // Chrome registers HTML/FileURL, not image-only promise types. An empty
    // HTML flavor enables entry without offering a text filename/clipping.
    if ([type isEqual:NSPasteboardTypeHTML]) return @"";
    if ([type isEqual:self.fileType]) return [self.owner chromeContents];
    if ([type isEqual:@"com.apple.pasteboard.promised-file-content-type"]) return self.fileType;
    if ([type isEqual:@"org.chromium.content-disposition"]) {
        NSCharacterSet *safe = [NSCharacterSet characterSetWithCharactersInString:@"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._-"];
        return [@"attachment; filename*=UTF-8''" stringByAppendingString:[self.owner.filename stringByAddingPercentEncodingWithAllowedCharacters:safe]];
    }
    return [super pasteboardPropertyListForType:type];
}
@end
@interface ImmichContents : NSObject <NSPasteboardItemDataProvider>
@property(strong) ImmichPromise *owner;
@end
@implementation ImmichContents
- (void)pasteboard:(NSPasteboard *)pasteboard item:(NSPasteboardItem *)item provideDataForType:(NSPasteboardType)type {
    // Finder/Firefox should fulfill the genuine file promise, not synchronously
    // fetch an image representation just to inspect the drag.
    if (!isChromiumDestination()) return;
    NSData *data = [self.owner chromeContents];
    if (data) [item setData:data forType:type];
}
@end
static BOOL addChromeContents(ImmichProvider *promise, NSPasteboard *pasteboard) {
    // AppKit must write the NSFilePromiseProvider itself to register a real
    // promise. Never clone its metadata into a generic NSPasteboardItem.
    if (!promise) return NO;
    NSPasteboardItem *item = pasteboard.pasteboardItems.firstObject;
    if (!item) return NO;
    ImmichContents *contents = [ImmichContents new]; contents.owner = promise.owner;
    // Keep the data provider alive via the promise's userInfo for this session.
    promise.userInfo = contents;
    return [item setDataProvider:contents forTypes:@[promise.fileType]];
}
static ImmichPromise *activeSource;
static id activeFilesSource;
@implementation ImmichPromise
- (void)dealloc { if (_context) _callbacks.release(_context); }
- (void)releaseIfFinished {
    @synchronized(self) {
        if (_context && _sessionEnded && (_fulfilled || _cancelled) && !_writers) {
            _callbacks.release(_context); _context = NULL;
            // The OS has finished requesting the representation. Release any
            // file-backed pasteboard NSData, without erasing a newer drag.
            NSInteger generation = _pasteboardGeneration;
            NSPasteboard *pasteboard = _pasteboard;
            dispatch_async(dispatch_get_main_queue(), ^{
                dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC), dispatch_get_main_queue(), ^{
                    if (pasteboard && generation >= 0 && pasteboard.changeCount == generation) [pasteboard clearContents];
                });
            });
        }
    }
}
- (NSString *)filePromiseProvider:(NSFilePromiseProvider *)provider fileNameForType:(NSString *)type {
    return self.filename;
}
- (NSData *)chromeContents {
    // Chrome does not fulfill NSFilePromiseProvider. Its macOS receiver asks for
    // the promised UTI's NSData synchronously, with a 256 MiB limit. Supply a
    // read-only file mapping: no full-original heap buffer or RAM cache here.
    void *context;
    @synchronized(self) {
        if (!_context) return nil;
        context = _callbacks.retain(_context); ++_writers;
    }
    ImmichCallbacks callbacks = self.callbacks;
    ImmichContentResult *result = [ImmichContentResult new]; result.condition = [NSCondition new];
    void *worker = callbacks.retain(context);
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        @autoreleasepool {
            char path[16384]; uint64_t length = 0; NSData *data = nil;
            BOOL cancelled = NO;
            while (callbacks.ready(worker) == 0) {
                [result.condition lock];
                cancelled = result.cancelled;
                if (!cancelled) [result.condition waitUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.02]];
                [result.condition unlock];
                if (cancelled) break;
            }
            if (!cancelled && callbacks.file_size(worker, &length) && length > 0 && length <= 256 * 1024 * 1024
                && callbacks.file_path(worker, path, sizeof(path))) {
                int fd = open(path, O_RDONLY);
                if (fd >= 0) {
                    void *bytes = mmap(NULL, (size_t)length, PROT_READ, MAP_PRIVATE, fd, 0); close(fd);
                    if (bytes != MAP_FAILED) {
                        void *mapping = callbacks.retain(worker);
                        data = [[NSData alloc] initWithBytesNoCopy:bytes length:(NSUInteger)length deallocator:^(void *p, NSUInteger n) {
                            munmap(p, n); callbacks.release(mapping);
                        }];
                    }
                }
            } else if (length > 256 * 1024 * 1024) {
                callbacks.failed(worker, "Chrome on macOS limits deferred file contents to 256 MiB.");
            }
            if (!data && length > 0 && length <= 256 * 1024 * 1024)
                callbacks.failed(worker, "Could not map the original from disk for Chrome.");
            [result.condition lock]; result.data = data; result.finished = YES;
            [result.condition broadcast]; [result.condition unlock];
            callbacks.release(worker);
        }
    });
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:30];
    NSData *data = nil;
    while (YES) {
        [result.condition lock];
        BOOL finished = result.finished; if (finished) data = result.data;
        [result.condition unlock];
        if (finished) break;
        if (deadline.timeIntervalSinceNow <= 0) {
            [result.condition lock]; result.cancelled = YES; [result.condition broadcast]; [result.condition unlock];
            callbacks.failed(context, "Chrome timed out waiting for the original. The latest download remains on disk for retry."); break;
        }
        // Do network/disk waiting on the worker. Service the source app's main
        // run loop while AppKit synchronously services Chrome's pasteboard RPC.
        if ([NSThread isMainThread]) [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
        else { [result.condition lock]; if (!result.finished) [result.condition waitUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.05]]; [result.condition unlock]; }
    }
    callbacks.release(context);
    @synchronized(self) {
        _browserReadFinished = YES;
        if (_sessionEnded && _chromiumDrop) _fulfilled = YES;
        --_writers;
    }
    [self releaseIfFinished]; return data;
}
- (NSOperationQueue *)operationQueueForFilePromiseProvider:(NSFilePromiseProvider *)provider {
    static NSOperationQueue *queue;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ queue = [NSOperationQueue new]; queue.maxConcurrentOperationCount = 2; });
    return queue;
}
- (void)filePromiseProvider:(NSFilePromiseProvider *)provider writePromiseToURL:(NSURL *)url
         completionHandler:(void (^)(NSError *))completion {
    // AppKit invokes this on our operation queue, never block the main thread.
    void *context;
    @synchronized(self) {
        if (!_context) {
            completion([NSError errorWithDomain:@"ImmichDesktop" code:2 userInfo:nil]); return;
        }
        context = _callbacks.retain(_context); ++_writers;
    }
    ImmichCallbacks callbacks = self.callbacks;
    int ok = callbacks.copy_file(context, url.fileSystemRepresentation);
    NSError *error = ok ? nil : [NSError errorWithDomain:@"ImmichDesktop" code:1
        userInfo:@{NSLocalizedDescriptionKey: @"The original could not be downloaded or delivered."}];
    completion(error);
    callbacks.release(context);
    @synchronized(self) { _fulfilled = YES; --_writers; }
    [self releaseIfFinished];
}
- (NSDragOperation)draggingSession:(NSDraggingSession *)session sourceOperationMaskForDraggingContext:(NSDraggingContext)context {
    return NSDragOperationCopy;
}
- (void)draggingSession:(NSDraggingSession *)session movedToPoint:(NSPoint)point {
    BOOL chromium = isChromiumDestination();
    if (chromium != self.chromiumDestination) {
        self.chromiumDestination = chromium;
        // A previous non-Chromium inspection may have found no image bytes.
        // Re-arm the lazy representation when entering Chromium's window.
        if (chromium) addChromeContents((ImmichProvider *)self.provider, session.draggingPasteboard);
    }
}
- (void)draggingSession:(NSDraggingSession *)session endedAtPoint:(NSPoint)point operation:(NSDragOperation)operation {
    [self finishDragAtPoint:point operation:operation];
}
- (void)finishDragAtPoint:(NSPoint)point operation:(NSDragOperation)operation {
    self.pasteboardGeneration = self.pasteboard ? self.pasteboard.changeCount : -1;
    @synchronized(self) {
        if (_context) _callbacks.ended(_context, operation != NSDragOperationNone);
        _sessionEnded = YES; _cancelled = operation == NSDragOperationNone;
        _chromiumDrop = !_cancelled && isChromiumDropPoint(point);
        if (_chromiumDrop && _browserReadFinished) _fulfilled = YES;
    }
    [self releaseIfFinished];
    activeSource = nil;
}
@end

int immich_mouse_down(void) { return ([NSEvent pressedMouseButtons] & 1) != 0; }
int immich_start_drag(void *handle, const char *name, const unsigned char *png, size_t png_len,
    const unsigned char *rgba, uint32_t width, uint32_t height, void *context, ImmichCallbacks callbacks) {
    ImmichPromise *source = [ImmichPromise new];
    (void)rgba;
    source.context = context; source.callbacks = callbacks;
    source.pasteboardGeneration = -1;
    source.filename = [NSString stringWithUTF8String:name];
    if (!immich_mouse_down() || activeSource || activeFilesSource || !source.filename) return 0;
    NSWindow *window = (__bridge NSWindow *)handle;
    NSView *view = window.contentView;
    NSImage *image = [[NSImage alloc] initWithData:[NSData dataWithBytes:png length:png_len]];
    if (!image || !view) return 0;
    UTType *type = [UTType typeWithFilenameExtension:source.filename.pathExtension];
    ImmichProvider *provider = [[ImmichProvider alloc] initWithFileType:(type ?: UTTypeData).identifier delegate:source];
    provider.owner = source;
    source.provider = provider;
    NSDraggingItem *item = [[NSDraggingItem alloc] initWithPasteboardWriter:provider];
    NSPoint point = [view convertPoint:window.mouseLocationOutsideOfEventStream fromView:nil];
    [item setDraggingFrame:NSMakeRect(point.x - width / 2.0, point.y - height / 2.0, width, height) contents:image];
    NSEvent *event = [NSEvent mouseEventWithType:NSEventTypeLeftMouseDragged location:window.mouseLocationOutsideOfEventStream
        modifierFlags:0 timestamp:NSApp.currentEvent.timestamp windowNumber:window.windowNumber
        context:nil eventNumber:0 clickCount:1 pressure:1];
    activeSource = source;
    @try {
        NSDraggingSession *session = [view beginDraggingSessionWithItems:@[item] event:event source:source];
        source.pasteboard = session.draggingPasteboard;
        addChromeContents(provider, session.draggingPasteboard);
        source.pasteboardGeneration = session.draggingPasteboard.changeCount;
    }
    @catch (NSException *exception) { activeSource = nil; return 0; }
    return 1;
}

// Chrome supports multiple existing file URLs, whereas its deferred-content
// compatibility path exposes only one file. The Rust outgoing store retains
// these files across drags/restarts; the receiver never sees an evictable path.
static NSArray<NSDraggingItem *> *fileDraggingItems(const char *const *paths, size_t count,
    NSImage *image, NSPoint point, uint32_t width, uint32_t height) {
    NSMutableArray *items = [NSMutableArray new];
    for (size_t index = 0; index < count; ++index) {
        NSString *path = [NSString stringWithUTF8String:paths[index]];
        if (!path || !path.isAbsolutePath || ![[NSFileManager defaultManager] fileExistsAtPath:path]) return nil;
        NSURL *url = [NSURL fileURLWithPath:path];
        NSDraggingItem *item = [[NSDraggingItem alloc] initWithPasteboardWriter:url];
        [item setDraggingFrame:NSMakeRect(point.x - width / 2.0, point.y - height / 2.0, width, height) contents:image];
        [items addObject:item];
    }
    return items;
}
@interface ImmichFilesSource : NSObject <NSDraggingSource>
@property void *context;
@property void (*finished)(void *, int);
- (void)finish:(BOOL)dropped;
@end
@implementation ImmichFilesSource
- (void)dealloc { if (_context) _finished(_context, 0); }
- (NSDragOperation)draggingSession:(NSDraggingSession *)session sourceOperationMaskForDraggingContext:(NSDraggingContext)context {
    return NSDragOperationCopy;
}
- (void)draggingSession:(NSDraggingSession *)session endedAtPoint:(NSPoint)point operation:(NSDragOperation)operation {
    [self finish:operation != NSDragOperationNone];
    NSPasteboard *pasteboard = session.draggingPasteboard;
    NSInteger generation = pasteboard.changeCount;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        if (pasteboard.changeCount == generation) [pasteboard clearContents];
    });
    activeFilesSource = nil;
}
- (void)finish:(BOOL)dropped {
    if (_context) { void *context = _context; _context = NULL; _finished(context, dropped); }
}
@end
int immich_start_files_drag(void *handle, const char *const *paths, size_t count,
    const unsigned char *png, size_t png_len, const unsigned char *rgba,
    uint32_t width, uint32_t height, void *context, void (*finished)(void *, int)) {
    (void)rgba;
    if (!count || !immich_mouse_down() || activeSource || activeFilesSource) return 0;
    NSWindow *window = (__bridge NSWindow *)handle;
    NSView *view = window.contentView;
    NSImage *image = [[NSImage alloc] initWithData:[NSData dataWithBytes:png length:png_len]];
    if (!view || !image) return 0;
    NSPoint point = [view convertPoint:window.mouseLocationOutsideOfEventStream fromView:nil];
    NSArray *items = fileDraggingItems(paths, count, image, point, width, height);
    if (items.count != count) return 0;
    NSEvent *event = [NSEvent mouseEventWithType:NSEventTypeLeftMouseDragged location:window.mouseLocationOutsideOfEventStream
        modifierFlags:0 timestamp:NSApp.currentEvent.timestamp windowNumber:window.windowNumber
        context:nil eventNumber:0 clickCount:1 pressure:1];
    ImmichFilesSource *source = [ImmichFilesSource new];
    source.context = context; source.finished = finished;
    activeFilesSource = source;
    @try {
        NSDraggingSession *session = [view beginDraggingSessionWithItems:items event:event source:source];
        session.draggingFormation = NSDraggingFormationStack;
    } @catch (NSException *exception) {
        source.context = NULL; activeFilesSource = nil; return 0;
    }
    return 1;
}

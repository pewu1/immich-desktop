// Off-screen native format/lifetime regression test. Uses a unique pasteboard,
// never the user's clipboard or the active drag pasteboard. No pointer synthesis.
#define IMMICH_NATIVE_TEST 1
#include "../src-tauri/native/drag_macos.m"
#include <stdatomic.h>
#include <sys/stat.h>

static atomic_int references;
static atomic_int polls;
static atomic_int copies;
static const char *fixture;
static CFAbsoluteTime readyAfter;
static void *keep(void *context) { atomic_fetch_add(&references, 1); return context; }
static void drop(void *context) { atomic_fetch_sub(&references, 1); }
static int sizeOfFile(void *context, uint64_t *size) {
    struct stat info; if (stat(fixture, &info)) return 0; *size = (uint64_t)info.st_size; return 1;
}
static int pathOfFile(void *context, char *buffer, size_t capacity) {
    if (strlen(fixture) >= capacity) return 0; strcpy(buffer, fixture); return 1;
}
static int readiness(void *context) { atomic_fetch_add(&polls, 1); return CFAbsoluteTimeGetCurrent() >= readyAfter; }
static void failure(void *context, const char *message) { fprintf(stderr, "%s\n", message); }
static void ended(void *context, int dropped) {}
static int copyFixture(void *context, const char *destination) {
    atomic_fetch_add(&copies, 1);
    while (!readiness(context)) usleep(1000);
    int input = open(fixture, O_RDONLY); if (input < 0) return 0;
    int output = open(destination, O_WRONLY | O_CREAT | O_EXCL, 0600);
    if (output < 0) { close(input); return 0; }
    unsigned char buffer[8192]; ssize_t count; int ok = 1;
    while ((count = read(input, buffer, sizeof(buffer))) > 0) {
        ssize_t offset = 0;
        while (offset < count) { ssize_t n = write(output, buffer + offset, count - offset); if (n <= 0) { ok = 0; break; } offset += n; }
        if (!ok) break;
    }
    if (count < 0) ok = 0;
    close(input); close(output); return ok;
}
int main(int argc, const char **argv) {
    if (argc < 2) return 2;
    BOOL baseline = argc > 2 && strcmp(argv[2], "--baseline") == 0;
    BOOL receiverRPC = baseline || (argc > 2 && strcmp(argv[2], "--receiver") == 0);
    fixture = argv[1]; atomic_init(&references, 1); atomic_init(&polls, 0); atomic_init(&copies, 0); readyAfter = CFAbsoluteTimeGetCurrent() + 0.25;
    @autoreleasepool {
        [NSApplication sharedApplication]; [NSApp setActivationPolicy:NSApplicationActivationPolicyProhibited]; [NSApp finishLaunching];
        ImmichCallbacks callbacks = {keep, drop, copyFixture, NULL, sizeOfFile, ended, pathOfFile, failure, readiness};
        ImmichPromise *source = [ImmichPromise new]; source.context = (void *)fixture; source.callbacks = callbacks; source.filename = @"drag-test.png";
        NSFilePromiseProvider *provider = baseline ? [[NSFilePromiseProvider alloc] initWithFileType:UTTypePNG.identifier delegate:source]
            : [[ImmichProvider alloc] initWithFileType:UTTypePNG.identifier delegate:source];
        if (!baseline) ((ImmichProvider *)provider).owner = source;
        NSPasteboard *board = [NSPasteboard pasteboardWithUniqueName];
        if (![board writeObjects:@[provider]]) return 3;
        NSData *expected = [NSData dataWithContentsOfFile:[NSString stringWithUTF8String:fixture]];
        if (!baseline) {
        if (!addChromeContents((ImmichProvider *)provider, board)) { fprintf(stderr, "cannot attach compatibility contents\n"); return 10; }
        testChromiumTarget = NO;
        if ([board dataForType:UTTypePNG.identifier] != nil || atomic_load(&polls) != 0) { fprintf(stderr, "FAIL: non-Chromium inspection rendered browser bytes\n"); return 13; }
        testChromiumTarget = YES;
        if (!addChromeContents((ImmichProvider *)provider, board)) return 10;
        if (atomic_load(&polls) != 0) { fprintf(stderr, "FAIL: original was rendered before the receiver requested it\n"); return 9; }
        NSArray *types = board.types;
        if ([types containsObject:NSPasteboardTypeString] || ![types containsObject:NSPasteboardTypeHTML] || ![types containsObject:UTTypePNG.identifier]) return 4;
        if (![[board stringForType:@"com.apple.pasteboard.promised-file-content-type"] isEqual:UTTypePNG.identifier]) return 5;
        if (![[board stringForType:@"org.chromium.content-disposition"] isEqual:@"attachment; filename*=UTF-8''drag-test.png"]) return 6;
        NSData *actual = [board dataForType:UTTypePNG.identifier];
        if (!actual || ![actual isEqualToData:expected]) { fprintf(stderr, "actual=%lu expected=%lu types=%s\n", (unsigned long)actual.length, (unsigned long)expected.length, types.description.UTF8String); return 7; }
        printf("PASS: Chrome-registered type, promised PNG bytes, filename metadata and delayed disk-backed rendering\n");
        testChromiumTarget = NO;
        [source finishDragAtPoint:NSZeroPoint operation:NSDragOperationCopy];
        if (!source.context) { fprintf(stderr, "FAIL: hovering Chromium released the promise before a Finder drop\n"); return 14; }
        }

        // Exercise the real receiver API used by Finder/native apps and
        // browsers that fulfill file promises, not just advertised type names.
        NSArray<NSFilePromiseReceiver *> *receivers = [board readObjectsForClasses:@[[NSFilePromiseReceiver class]] options:@{}];
        if (receivers.count != 1) { fprintf(stderr, "FAIL: expected one genuine file promise, got %lu\n", (unsigned long)receivers.count); return 11; }
        NSString *folder = [NSTemporaryDirectory() stringByAppendingPathComponent:[[NSUUID UUID] UUIDString]];
        NSURL *destination = [NSURL fileURLWithPath:folder isDirectory:YES];
        [[NSFileManager defaultManager] createDirectoryAtURL:destination withIntermediateDirectories:NO attributes:nil error:nil];
        NSOperationQueue *queue = [NSOperationQueue new];
        NSCondition *completed = [NSCondition new]; __block BOOL done = NO, validFile = NO;
        readyAfter = CFAbsoluteTimeGetCurrent() + 0.25;
        void (^reader)(NSURL *, NSError *) = ^(NSURL *url, NSError *error) {
            NSData *received = error ? nil : [NSData dataWithContentsOfURL:url];
            [completed lock];
            validFile = !error && [url.lastPathComponent isEqual:@"drag-test.png"] && [received isEqualToData:expected];
            if (error) fprintf(stderr, "file promise failed: %s\n", error.description.UTF8String);
            done = YES; [completed broadcast]; [completed unlock];
        };
        if (receiverRPC) {
            [receivers.firstObject receivePromisedFilesAtDestination:destination options:@{} operationQueue:queue reader:reader];
        } else {
            NSURL *file = [destination URLByAppendingPathComponent:[source filePromiseProvider:provider fileNameForType:provider.fileType]];
            [queue addOperationWithBlock:^{ [source filePromiseProvider:provider writePromiseToURL:file completionHandler:^(NSError *error) { reader(file, error); }]; }];
        }
        NSDate *finishBy = [NSDate dateWithTimeIntervalSinceNow:8];
        while (finishBy.timeIntervalSinceNow > 0) {
            [completed lock]; BOOL finished = done; [completed unlock]; if (finished) break;
            [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
        }
        [completed lock]; BOOL success = done && validFile; [completed unlock];
        if (!success) { fprintf(stderr, "FAIL: Finder-style receiver did not receive the original file (writes=%d)\n", atomic_load(&copies)); return 12; }
        [[NSFileManager defaultManager] removeItemAtURL:destination error:nil];
        puts(receiverRPC ? "PASS: real receiver writes the original" : "PASS: genuine promise recognized; native delegate writes the named original, not text");
        [board clearContents]; [board releaseGlobally];
    }
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:2];
    while (atomic_load(&references) != 0 && deadline.timeIntervalSinceNow > 0)
        @autoreleasepool { [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.01]]; }
    if (atomic_load(&references) != 0) { fprintf(stderr, "FAIL: native transfer references not released: %d\n", atomic_load(&references)); return 8; }
    puts("PASS: native mapping/provider references released"); return 0;
}

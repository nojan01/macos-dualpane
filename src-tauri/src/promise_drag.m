#import <Cocoa/Cocoa.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#include "promise_drag.h"

static db_drop_callback g_callback = NULL;
static NSMutableDictionary<NSNumber *, void (^)(NSError *)> *g_completions = nil;
static NSMutableDictionary<NSNumber *, NSString *> *g_sources = nil;
static NSMutableDictionary<NSNumber *, NSURL *> *g_destinations = nil;
static NSMutableArray *g_active_delegates = nil;
static uint64_t g_next_id = 1;

void db_set_drop_callback(db_drop_callback cb) { g_callback = cb; }

static void ensure_globals(void) {
    if (g_completions == nil) g_completions = [NSMutableDictionary dictionary];
    if (g_sources == nil) g_sources = [NSMutableDictionary dictionary];
    if (g_destinations == nil) g_destinations = [NSMutableDictionary dictionary];
    if (g_active_delegates == nil) g_active_delegates = [NSMutableArray array];
}

@interface DBPromiseDelegate : NSObject <NSFilePromiseProviderDelegate>
@property (strong) NSString *sourcePath;
@property (assign) uint64_t dropId;
@end

@implementation DBPromiseDelegate
- (NSString *)filePromiseProvider:(NSFilePromiseProvider *)provider
                  fileNameForType:(NSString *)fileType {
    NSString *name = [self.sourcePath lastPathComponent];
    NSLog(@"[DualBeam] fileNameForType type=%@ -> %@", fileType, name);
    return name;
}

- (void)filePromiseProvider:(NSFilePromiseProvider *)provider
           writePromiseToURL:(NSURL *)url
           completionHandler:(void (^)(NSError *_Nullable))completionHandler {
    NSLog(@"[DualBeam] writePromiseToURL url=%@ src=%@", url, self.sourcePath);
    uint64_t dropId = self.dropId;
    @synchronized (g_completions) {
        g_completions[@(dropId)] = [completionHandler copy];
        g_destinations[@(dropId)] = url;
    }
    if (g_callback) {
        const char *src = [self.sourcePath UTF8String];
        const char *dst = [[url path] UTF8String];
        g_callback(dropId, src, dst);
        return;
    }
    // Fallback: no callback registered — copy directly.
    NSError *err = nil;
    [[NSFileManager defaultManager] removeItemAtURL:url error:nil];
    [[NSFileManager defaultManager] copyItemAtPath:self.sourcePath
                                            toPath:[url path]
                                             error:&err];
    completionHandler(err);
    @synchronized (g_completions) {
        [g_completions removeObjectForKey:@(dropId)];
        [g_destinations removeObjectForKey:@(dropId)];
        [g_sources removeObjectForKey:@(dropId)];
    }
}

- (NSOperationQueue *)operationQueueForFilePromiseProvider:(NSFilePromiseProvider *)provider {
    static NSOperationQueue *q = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ q = [[NSOperationQueue alloc] init]; });
    return q;
}
@end

@interface DBDragSource : NSObject <NSDraggingSource>
@end

@implementation DBDragSource
- (NSDragOperation)draggingSession:(NSDraggingSession *)session
    sourceOperationMaskForDraggingContext:(NSDraggingContext)context {
    NSLog(@"[DualBeam] sourceOperationMask ctx=%ld", (long)context);
    return NSDragOperationCopy;
}

- (void)draggingSession:(NSDraggingSession *)session
       willBeginAtPoint:(NSPoint)screenPoint {
    NSPasteboard *pb = [session draggingPasteboard];
    NSString *typesStr = [[pb types] componentsJoinedByString:@","];
    NSLog(@"[DualBeam] willBegin at (%f,%f) pbTypes=[%@] pbItems=%lu",
          screenPoint.x, screenPoint.y, typesStr, (unsigned long)[[pb pasteboardItems] count]);
    NSUInteger idx = 0;
    for (NSPasteboardItem *it in [pb pasteboardItems]) {
        NSString *its = [[it types] componentsJoinedByString:@","];
        NSLog(@"[DualBeam]   item[%lu] types=[%@]", (unsigned long)idx++, its);
    }
}

- (void)draggingSession:(NSDraggingSession *)session
           movedToPoint:(NSPoint)screenPoint {
    // (no log — too noisy)
}

- (void)draggingSession:(NSDraggingSession *)session
           endedAtPoint:(NSPoint)screenPoint
              operation:(NSDragOperation)operation {
    NSLog(@"[DualBeam] draggingSession ended at (%f,%f) op=%lu",
          screenPoint.x, screenPoint.y, (unsigned long)operation);
    // Active delegates have done their job; release strong refs.
    @synchronized (g_active_delegates) {
        [g_active_delegates removeAllObjects];
    }
}
@end

static DBDragSource *g_drag_source = nil;

static NSString *utiForPath(NSString *path) {
    NSString *ext = [path pathExtension];
    if (ext.length == 0) return @"public.data";
    if (@available(macOS 11.0, *)) {
        UTType *t = [UTType typeWithFilenameExtension:ext];
        return t ? t.identifier : @"public.data";
    }
    return @"public.data";
}

int db_start_promise_drag(const char *const *paths, int count, const char **out_err) {
    __block int retval = 0;
    __block NSString *errstr = nil;

    NSMutableArray<NSString *> *pathList = [NSMutableArray array];
    for (int i = 0; i < count; i++) {
        [pathList addObject:[NSString stringWithUTF8String:paths[i]]];
    }

    dispatch_block_t work = ^{
        @try {
            ensure_globals();
            NSLog(@"[DualBeam] db_start_promise_drag paths=%lu", (unsigned long)[pathList count]);
            if (g_drag_source == nil) g_drag_source = [[DBDragSource alloc] init];

            NSWindow *win = [[NSApplication sharedApplication] keyWindow];
            if (!win) win = [[[NSApplication sharedApplication] orderedWindows] firstObject];
            if (!win) { errstr = @"no window"; retval = -1; return; }
            NSLog(@"[DualBeam] window=%@ number=%ld", win, (long)[win windowNumber]);

            NSView *contentView = [win contentView];
            // Drag-Session immer auf der contentView starten (so macht es auch
            // tauri-plugin-drag) — auf einer tieferen WKWebView-Subview schluckt
            // die WebKit-Hit-Test-Logik den Drag.
            NSView *dragView = contentView;

            NSEvent *event = [NSApp currentEvent];
            BOOL eventUsable = event != nil &&
                ([event type] == NSEventTypeLeftMouseDown ||
                 [event type] == NSEventTypeLeftMouseDragged);
            NSPoint locInWindow = [win mouseLocationOutsideOfEventStream];
            if (!eventUsable) {
                NSTimeInterval ts = event ? [event timestamp]
                                          : [[NSProcessInfo processInfo] systemUptime];
                event = [NSEvent mouseEventWithType:NSEventTypeLeftMouseDragged
                                           location:locInWindow
                                      modifierFlags:0
                                          timestamp:ts
                                       windowNumber:[win windowNumber]
                                            context:nil
                                        eventNumber:0
                                         clickCount:1
                                           pressure:1.0];
                if (!event) { errstr = @"could not synthesize event"; retval = -2; return; }
            }

            NSMutableArray *items = [NSMutableArray array];
            for (NSUInteger i = 0; i < [pathList count]; i++) {
                NSString *path = pathList[i];
                NSString *uti = utiForPath(path);

                DBPromiseDelegate *del = [[DBPromiseDelegate alloc] init];
                del.sourcePath = path;
                del.dropId = g_next_id++;
                @synchronized (g_active_delegates) { [g_active_delegates addObject:del]; }
                @synchronized (g_completions) { g_sources[@(del.dropId)] = path; }

                NSFilePromiseProvider *prov =
                    [[NSFilePromiseProvider alloc] initWithFileType:uti delegate:del];

                NSDraggingItem *item =
                    [[NSDraggingItem alloc] initWithPasteboardWriter:prov];

                NSImage *icon = [[NSWorkspace sharedWorkspace] iconForFile:path];
                if (!icon) icon = [NSImage imageNamed:NSImageNameMultipleDocuments];
                NSLog(@"[DualBeam] item[%lu] path=%@ uti=%@ icon=%@",
                      (unsigned long)i, path, uti, icon);
                CGFloat sz = 32.0;
                NSRect frame = NSMakeRect(locInWindow.x - sz / 2.0 + ((CGFloat)i * 4),
                                          locInWindow.y - sz / 2.0 - ((CGFloat)i * 4),
                                          sz, sz);
                [item setDraggingFrame:frame contents:icon];
                [items addObject:item];
            }

            NSDraggingSession *session =
                [dragView beginDraggingSessionWithItems:items
                                                  event:event
                                                 source:g_drag_source];
            NSLog(@"[DualBeam] beginDraggingSession view=%@ items=%lu evType=%lu locInWin=(%f,%f) session=%@",
                  dragView, (unsigned long)[items count], (unsigned long)[event type],
                  locInWindow.x, locInWindow.y, session);
            NSPasteboard *pb = [session draggingPasteboard];
            NSString *typesStr = [[pb types] componentsJoinedByString:@","];
            NSLog(@"[DualBeam] session pbTypes=[%@] pbItemCount=%lu",
                  typesStr, (unsigned long)[[pb pasteboardItems] count]);
            NSUInteger idx = 0;
            for (NSPasteboardItem *it in [pb pasteboardItems]) {
                NSString *its = [[it types] componentsJoinedByString:@","];
                NSLog(@"[DualBeam]   item[%lu] types=[%@]", (unsigned long)idx++, its);
            }
        } @catch (NSException *ex) {
            errstr = [ex reason] ?: @"objc exception";
            retval = -3;
        }
    };

    if ([NSThread isMainThread]) work();
    else dispatch_sync(dispatch_get_main_queue(), work);

    if (retval != 0 && out_err) {
        const char *u = [errstr UTF8String];
        *out_err = strdup(u ? u : "unknown error");
    }
    return retval;
}

int db_resolve_promise(uint64_t dropId, int action, const char **out_err) {
    __block int retval = 0;
    __block NSString *errstr = nil;

    dispatch_block_t work = ^{
        void (^completion)(NSError *) = nil;
        NSURL *destURL = nil;
        NSString *srcPath = nil;
        @synchronized (g_completions) {
            completion = g_completions[@(dropId)];
            destURL = g_destinations[@(dropId)];
            srcPath = g_sources[@(dropId)];
            [g_completions removeObjectForKey:@(dropId)];
            [g_destinations removeObjectForKey:@(dropId)];
            [g_sources removeObjectForKey:@(dropId)];
        }
        if (!completion || !destURL || !srcPath) {
            errstr = @"unknown drop id";
            retval = -1;
            return;
        }

        if (action == 1) {
            NSError *cancelErr = [NSError errorWithDomain:@"DualBeam"
                                                     code:1
                                                 userInfo:@{NSLocalizedDescriptionKey: @"Cancelled by user"}];
            completion(cancelErr);
            return;
        }

        NSError *err = nil;
        NSString *destPath = [destURL path];

        if (action == 0) {
            // overwrite
            [[NSFileManager defaultManager] removeItemAtPath:destPath error:nil];
            [[NSFileManager defaultManager] copyItemAtPath:srcPath toPath:destPath error:&err];
        } else if (action == 2) {
            // keep both: find a non-colliding name in same directory.
            NSString *finalPath = destPath;
            if ([[NSFileManager defaultManager] fileExistsAtPath:finalPath]) {
                NSString *dir = [destPath stringByDeletingLastPathComponent];
                NSString *base = [destPath lastPathComponent];
                NSString *stem = [base stringByDeletingPathExtension];
                NSString *ext = [base pathExtension];
                int i = 2;
                while (i < 10000) {
                    NSString *candidate;
                    if (ext.length > 0) {
                        candidate = [NSString stringWithFormat:@"%@/%@ %d.%@", dir, stem, i, ext];
                    } else {
                        candidate = [NSString stringWithFormat:@"%@/%@ %d", dir, stem, i];
                    }
                    if (![[NSFileManager defaultManager] fileExistsAtPath:candidate]) {
                        finalPath = candidate;
                        break;
                    }
                    i++;
                }
            }
            [[NSFileManager defaultManager] copyItemAtPath:srcPath toPath:finalPath error:&err];
        } else {
            // unknown action: treat as cancel
            NSError *cancelErr = [NSError errorWithDomain:@"DualBeam"
                                                     code:2
                                                 userInfo:@{NSLocalizedDescriptionKey: @"Invalid action"}];
            completion(cancelErr);
            return;
        }

        completion(err);
    };

    if ([NSThread isMainThread]) work();
    else dispatch_sync(dispatch_get_main_queue(), work);

    if (retval != 0 && out_err) {
        const char *u = [errstr UTF8String];
        *out_err = strdup(u ? u : "unknown error");
    }
    return retval;
}

int db_clipboard_write_files(const char *const *paths, int count, const char **out_err) {
    __block int retval = 0;
    __block NSString *errstr = nil;

    NSMutableArray<NSURL *> *urls = [NSMutableArray array];
    for (int i = 0; i < count; i++) {
        NSString *p = [NSString stringWithUTF8String:paths[i]];
        if (p.length == 0) continue;
        NSURL *u = [NSURL fileURLWithPath:p];
        if (u) [urls addObject:u];
    }
    if (urls.count == 0) {
        if (out_err) *out_err = strdup("no paths");
        return -1;
    }

    dispatch_block_t work = ^{
        @try {
            NSLog(@"[DualBeam] db_clipboard_write_files urls=%lu first=%@", (unsigned long)urls.count, urls.firstObject);
            NSPasteboard *pb = [NSPasteboard generalPasteboard];
            [pb clearContents];
            BOOL ok = [pb writeObjects:urls];
            NSLog(@"[DualBeam] writeObjects ok=%d pbTypes=%@", ok, [[pb types] componentsJoinedByString:@","]);
            if (!ok) { errstr = @"writeObjects failed"; retval = -2; }
        } @catch (NSException *ex) {
            errstr = [ex reason] ?: @"objc exception";
            retval = -3;
        }
    };
    if ([NSThread isMainThread]) work();
    else dispatch_sync(dispatch_get_main_queue(), work);

    if (retval != 0 && out_err) {
        const char *u = [errstr UTF8String];
        *out_err = strdup(u ? u : "unknown error");
    }
    return retval;
}

int db_clipboard_read_files(char ***out_paths, const char **out_err) {
    __block int retval = 0;
    __block NSString *errstr = nil;
    __block NSArray<NSURL *> *urls = nil;

    dispatch_block_t work = ^{
        @try {
            NSPasteboard *pb = [NSPasteboard generalPasteboard];
            NSDictionary *opts = @{ NSPasteboardURLReadingFileURLsOnlyKey: @YES };
            NSArray *objs = [pb readObjectsForClasses:@[[NSURL class]] options:opts];
            urls = objs ?: @[];
            NSLog(@"[DualBeam] db_clipboard_read_files urls=%lu pbTypes=%@",
                  (unsigned long)urls.count, [[pb types] componentsJoinedByString:@","]);
        } @catch (NSException *ex) {
            errstr = [ex reason] ?: @"objc exception";
            retval = -3;
        }
    };
    if ([NSThread isMainThread]) work();
    else dispatch_sync(dispatch_get_main_queue(), work);

    if (retval != 0) {
        if (out_err) *out_err = strdup([errstr UTF8String] ?: "unknown error");
        return retval;
    }

    int n = (int)urls.count;
    if (n == 0) {
        *out_paths = NULL;
        return 0;
    }
    char **arr = (char **)malloc(sizeof(char *) * (size_t)n);
    for (int i = 0; i < n; i++) {
        NSURL *u = urls[i];
        const char *p = [[u path] UTF8String];
        arr[i] = strdup(p ? p : "");
    }
    *out_paths = arr;
    return n;
}

void db_set_dock_badge(const char *label) {
    NSString *text = (label && label[0] != '\0')
        ? [NSString stringWithUTF8String:label]
        : nil;
    dispatch_block_t work = ^{
        [[NSApp dockTile] setBadgeLabel:text];
    };
    if ([NSThread isMainThread]) work();
    else dispatch_async(dispatch_get_main_queue(), work);
}

int db_file_icon_png(const char *path, int size, unsigned char **out_png,
                     int *out_len, const char **out_err) {
    if (out_png) *out_png = NULL;
    if (out_len) *out_len = 0;
    if (out_err) *out_err = NULL;
    if (path == NULL || out_png == NULL || out_len == NULL) {
        if (out_err) *out_err = strdup("invalid arguments");
        return -1;
    }
    if (size <= 0) size = 32;

    __block int retval = 0;
    __block NSData *pngData = nil;
    __block NSString *errstr = nil;

    dispatch_block_t work = ^{
        @autoreleasepool {
            @try {
                NSString *p = [NSString stringWithUTF8String:path];
                NSImage *icon = [[NSWorkspace sharedWorkspace] iconForFile:p];
                if (!icon) {
                    icon = [NSImage imageNamed:NSImageNameMultipleDocuments];
                }
                if (!icon) {
                    errstr = @"no icon";
                    retval = -2;
                    return;
                }
                NSSize target = NSMakeSize((CGFloat)size, (CGFloat)size);
                // Draw the icon into a fixed-size ARGB bitmap so the resulting
                // PNG always has the requested pixel dimensions.
                NSBitmapImageRep *rep = [[NSBitmapImageRep alloc]
                    initWithBitmapDataPlanes:NULL
                                  pixelsWide:size
                                  pixelsHigh:size
                               bitsPerSample:8
                             samplesPerPixel:4
                                    hasAlpha:YES
                                    isPlanar:NO
                              colorSpaceName:NSCalibratedRGBColorSpace
                                 bytesPerRow:0
                                bitsPerPixel:0];
                rep.size = target;
                NSGraphicsContext *ctx =
                    [NSGraphicsContext graphicsContextWithBitmapImageRep:rep];
                [NSGraphicsContext saveGraphicsState];
                [NSGraphicsContext setCurrentContext:ctx];
                [icon drawInRect:NSMakeRect(0, 0, target.width, target.height)
                        fromRect:NSZeroRect
                       operation:NSCompositingOperationSourceOver
                        fraction:1.0];
                [NSGraphicsContext restoreGraphicsState];
                pngData = [rep representationUsingType:NSBitmapImageFileTypePNG
                                            properties:@{}];
                if (!pngData) {
                    errstr = @"png encode failed";
                    retval = -3;
                }
            } @catch (NSException *ex) {
                errstr = [ex reason] ?: @"objc exception";
                retval = -4;
            }
        }
    };
    if ([NSThread isMainThread]) work();
    else dispatch_sync(dispatch_get_main_queue(), work);

    if (retval != 0) {
        if (out_err) *out_err = strdup([errstr UTF8String] ?: "unknown error");
        return retval;
    }

    NSUInteger len = [pngData length];
    unsigned char *buf = (unsigned char *)malloc(len);
    if (!buf) {
        if (out_err) *out_err = strdup("oom");
        return -5;
    }
    memcpy(buf, [pngData bytes], len);
    *out_png = buf;
    *out_len = (int)len;
    return 0;
}

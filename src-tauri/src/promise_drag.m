#import <Cocoa/Cocoa.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#import <Quartz/Quartz.h>
#import <objc/runtime.h>
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

int db_open_with_apps(const char *path, char **out_json, const char **out_err) {
    if (out_json) *out_json = NULL;
    if (out_err) *out_err = NULL;
    if (!path || !out_json) {
        if (out_err) *out_err = strdup("invalid arguments");
        return -1;
    }

    __block int retval = 0;
    __block NSString *errstr = nil;
    __block NSString *json = nil;
    NSString *file = [NSString stringWithUTF8String:path];

    dispatch_block_t work = ^{
        @try {
            NSURL *url = [NSURL fileURLWithPath:file];
            NSWorkspace *ws = [NSWorkspace sharedWorkspace];
            NSURL *defaultApp = [ws URLForApplicationToOpenURL:url];
            NSArray<NSURL *> *apps = [ws URLsForApplicationsToOpenURL:url] ?: @[];

            NSFileManager *fm = [NSFileManager defaultManager];
            NSMutableArray *items = [NSMutableArray array];
            NSMutableSet<NSString *> *seen = [NSMutableSet set];
            for (NSURL *app in apps) {
                NSString *appPath = [app path];
                if (appPath.length == 0) continue;
                // Dieselbe App kann mehrfach registriert sein (z. B. zwei
                // Versionen am selben Ort); doppelte Einträge im Menü wären
                // für den Nutzer nicht unterscheidbar.
                if ([seen containsObject:appPath]) continue;
                [seen addObject:appPath];
                NSString *name = [fm displayNameAtPath:appPath];
                if (name.length == 0) name = [appPath lastPathComponent];
                BOOL isDefault = defaultApp != nil &&
                    [[defaultApp path] isEqualToString:appPath];
                [items addObject:@{
                    @"name": name,
                    @"path": appPath,
                    @"isDefault": @(isDefault),
                }];
            }

            // Standardprogramm zuerst, danach alphabetisch nach Anzeigename –
            // die Reihenfolge von LaunchServices ist für Menschen willkürlich.
            [items sortUsingComparator:^NSComparisonResult(NSDictionary *a, NSDictionary *b) {
                BOOL da = [a[@"isDefault"] boolValue];
                BOOL db_ = [b[@"isDefault"] boolValue];
                if (da != db_) return da ? NSOrderedAscending : NSOrderedDescending;
                return [a[@"name"] localizedStandardCompare:b[@"name"]];
            }];

            NSError *jsonErr = nil;
            NSData *data = [NSJSONSerialization dataWithJSONObject:items
                                                           options:0
                                                             error:&jsonErr];
            if (data == nil) {
                errstr = [jsonErr localizedDescription] ?: @"json encoding failed";
                retval = -2;
            } else {
                json = [[NSString alloc] initWithData:data
                                             encoding:NSUTF8StringEncoding];
            }
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
    *out_json = strdup([json UTF8String] ?: "[]");
    return 0;
}

int db_open_with(const char *const *paths, int count, const char *app_path,
                 const char **out_err) {
    if (out_err) *out_err = NULL;
    if (!paths || count <= 0 || !app_path) {
        if (out_err) *out_err = strdup("invalid arguments");
        return -1;
    }

    NSMutableArray<NSURL *> *urls = [NSMutableArray arrayWithCapacity:(NSUInteger)count];
    for (int i = 0; i < count; i++) {
        if (!paths[i]) continue;
        NSString *p = [NSString stringWithUTF8String:paths[i]];
        if (p.length == 0) continue;
        [urls addObject:[NSURL fileURLWithPath:p]];
    }
    if (urls.count == 0) {
        if (out_err) *out_err = strdup("no valid paths");
        return -1;
    }
    NSURL *app = [NSURL fileURLWithPath:[NSString stringWithUTF8String:app_path]];

    __block int retval = 0;
    __block NSString *errstr = nil;
    // `openURLs:` meldet das Ergebnis asynchron. Ohne Warten wüsste der Nutzer
    // bei einer beschädigten App nie, dass das Öffnen fehlgeschlagen ist.
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    dispatch_block_t work = ^{
        @try {
            NSWorkspaceOpenConfiguration *cfg =
                [NSWorkspaceOpenConfiguration configuration];
            [[NSWorkspace sharedWorkspace]
                        openURLs:urls
            withApplicationAtURL:app
                   configuration:cfg
               completionHandler:^(NSRunningApplication *running, NSError *error) {
                   (void)running;
                   if (error != nil) {
                       errstr = [error localizedDescription] ?: @"open failed";
                       retval = -2;
                   }
                   dispatch_semaphore_signal(done);
               }];
        } @catch (NSException *ex) {
            errstr = [ex reason] ?: @"objc exception";
            retval = -3;
            dispatch_semaphore_signal(done);
        }
    };

    if ([NSThread isMainThread]) {
        // Auf dem Hauptthread darf nicht gewartet werden: Käme der
        // Abschlussblock ebenfalls auf der Hauptschleife an, stünden beide
        // Seiten still. Hier wird nur angestoßen; ein Fehler bliebe ungemeldet.
        work();
        return 0;
    }

    dispatch_async(dispatch_get_main_queue(), work);

    // Zeitgrenze, damit ein hängender LaunchServices-Aufruf den aufrufenden
    // Tauri-Befehl nicht dauerhaft blockiert.
    if (dispatch_semaphore_wait(done,
            dispatch_time(DISPATCH_TIME_NOW, (int64_t)(30 * NSEC_PER_SEC))) != 0) {
        if (out_err) *out_err = strdup("Zeitüberschreitung beim Öffnen");
        return -4;
    }

    if (retval != 0) {
        if (out_err) *out_err = strdup([errstr UTF8String] ?: "unknown error");
        return retval;
    }
    return 0;
}

int db_choose_application(char **out_path, const char **out_err) {
    if (out_path) *out_path = NULL;
    if (out_err) *out_err = NULL;
    if (!out_path) {
        if (out_err) *out_err = strdup("invalid arguments");
        return -1;
    }

    __block int retval = 0;
    __block NSString *picked = nil;
    __block NSString *errstr = nil;

    dispatch_block_t work = ^{
        @try {
            NSOpenPanel *panel = [NSOpenPanel openPanel];
            // Ein .app-Bündel ist im Dateisystem ein Verzeichnis. Ohne
            // treatsFilePackagesAsDirectories:NO würde der Dialog hineinnavigieren,
            // statt es auswählbar zu machen.
            panel.canChooseFiles = YES;
            panel.canChooseDirectories = NO;
            panel.treatsFilePackagesAsDirectories = NO;
            panel.allowsMultipleSelection = NO;
            panel.allowedContentTypes = @[ UTTypeApplicationBundle ];
            panel.directoryURL = [NSURL fileURLWithPath:@"/Applications"];

            // Ohne Aktivieren erscheint der Dialog unter Umständen hinter dem
            // Fenster, aus dem er angefordert wurde.
            [NSApp activateIgnoringOtherApps:YES];
            if ([panel runModal] == NSModalResponseOK) {
                picked = [[panel URL] path];
                if (picked.length == 0) retval = 1;
            } else {
                retval = 1;
            }
        } @catch (NSException *ex) {
            errstr = [ex reason] ?: @"objc exception";
            retval = -3;
        }
    };

    if ([NSThread isMainThread]) {
        work();
    } else {
        // Ohne Zeitgrenze: Der Dialog wartet auf den Nutzer, und der darf sich
        // Zeit lassen. Der Hauptthread arbeitet den Block sicher ab, weil hier
        // ausdrücklich nicht von ihm aus gewartet wird.
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        dispatch_async(dispatch_get_main_queue(), ^{
            work();
            dispatch_semaphore_signal(done);
        });
        dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
    }

    if (retval < 0) {
        if (out_err) *out_err = strdup([errstr UTF8String] ?: "unknown error");
        return retval;
    }
    if (retval == 1) return 1;
    *out_path = strdup([picked UTF8String] ?: "");
    return 0;
}

int db_set_default_application(const char *app_path, const char *file_path,
                               const char **out_err) {
    if (out_err) *out_err = NULL;
    if (!app_path || !file_path) {
        if (out_err) *out_err = strdup("invalid arguments");
        return -1;
    }

    NSURL *app = [NSURL fileURLWithPath:[NSString stringWithUTF8String:app_path]];
    NSURL *file = [NSURL fileURLWithPath:[NSString stringWithUTF8String:file_path]];

    __block int retval = 0;
    __block NSString *errstr = nil;
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    dispatch_block_t work = ^{
        @try {
            [[NSWorkspace sharedWorkspace]
                setDefaultApplicationAtURL:app
                          toOpenFileAtURL:file
                        completionHandler:^(NSError *error) {
                            if (error != nil) {
                                errstr = [error localizedDescription]
                                    ?: @"could not set default application";
                                retval = -2;
                            }
                            dispatch_semaphore_signal(done);
                        }];
        } @catch (NSException *ex) {
            errstr = [ex reason] ?: @"objc exception";
            retval = -3;
            dispatch_semaphore_signal(done);
        }
    };

    if ([NSThread isMainThread]) {
        // Wie bei db_open_with: Auf dem Hauptthread wird nur angestoßen, sonst
        // stünde das Warten dem eigenen Abschlussblock im Weg.
        work();
        return 0;
    }

    dispatch_async(dispatch_get_main_queue(), work);
    if (dispatch_semaphore_wait(done,
            dispatch_time(DISPATCH_TIME_NOW, (int64_t)(30 * NSEC_PER_SEC))) != 0) {
        if (out_err) *out_err = strdup("Zeitüberschreitung beim Festlegen");
        return -4;
    }

    if (retval != 0) {
        if (out_err) *out_err = strdup([errstr UTF8String] ?: "unknown error");
        return retval;
    }
    return 0;
}

void db_set_dock_badge(const char *label) {    NSString *text = (label && label[0] != '\0')
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

#pragma mark - Edit menu cleanup

// macOS automatically injects text-service items (AutoFill, Writing Tools,
// Substitutions, Transformations, Speech, Emoji & Symbols, Start Dictation)
// into any standard Edit menu. We keep only the classic editing commands.
static BOOL db_is_kept_edit_item(NSMenuItem *item) {
    if (item.isSeparatorItem) return NO;
    SEL a = item.action;
    if (a == NULL) return NO; // submenu parents like AutoFill / Writing Tools
    static NSArray<NSString *> *keep = nil;
    if (keep == nil) {
        keep = @[ @"undo:", @"redo:", @"cut:", @"copy:", @"paste:",
                  @"pasteAsPlainText:", @"delete:", @"selectAll:" ];
    }
    return [keep containsObject:NSStringFromSelector(a)];
}

static void db_trim_edit_menu(NSMenu *menu) {
    if (menu == nil) return;
    NSArray<NSMenuItem *> *items = [menu.itemArray copy];
    for (NSMenuItem *item in items) {
        if (!db_is_kept_edit_item(item)) {
            [menu removeItem:item];
        }
    }
}

@interface DBEditMenuCleaner : NSObject <NSMenuDelegate>
@property (weak) id<NSMenuDelegate> original;
@end

@implementation DBEditMenuCleaner
- (void)menuNeedsUpdate:(NSMenu *)menu {
    if ([self.original respondsToSelector:@selector(menuNeedsUpdate:)]) {
        [self.original menuNeedsUpdate:menu];
    }
    db_trim_edit_menu(menu);
}
@end

static DBEditMenuCleaner *g_edit_cleaner = nil;

static NSMenu *db_find_edit_menu(void) {
    NSMenu *main = [NSApp mainMenu];
    if (main == nil) return nil;
    for (NSMenuItem *top in main.itemArray) {
        NSMenu *sub = top.submenu;
        if (sub == nil) continue;
        for (NSMenuItem *it in sub.itemArray) {
            if (it.action == @selector(paste:)) return sub;
        }
    }
    return nil;
}

void db_clean_edit_menu(void) {
    void (^work)(void) = ^{
        NSUserDefaults *d = [NSUserDefaults standardUserDefaults];
        [d setBool:YES forKey:@"NSDisabledDictationMenuItem"];
        [d setBool:YES forKey:@"NSDisabledCharacterPaletteMenuItem"];

        NSMenu *edit = db_find_edit_menu();
        if (edit == nil) return;
        db_trim_edit_menu(edit);
        if (g_edit_cleaner == nil) {
            g_edit_cleaner = [[DBEditMenuCleaner alloc] init];
        }
        if (edit.delegate != (id)g_edit_cleaner) {
            g_edit_cleaner.original = edit.delegate;
            edit.delegate = g_edit_cleaner;
        }
    };
    if ([NSThread isMainThread]) work();
    else dispatch_sync(dispatch_get_main_queue(), work);
}

#pragma mark - Dock menu

// Right-clicking the Dock icon shows a menu. macOS queries the app delegate's
// -applicationDockMenu: method for it. wry's delegate does not implement that
// method, so we add it at runtime and return our own menu with a
// "New Window" entry, mirroring Finder's behaviour.
static db_dock_callback g_dock_cb = NULL;
static NSString *g_dock_title = nil;

@interface DBDockTarget : NSObject
- (void)dbNewWindow:(id)sender;
@end

@implementation DBDockTarget
- (void)dbNewWindow:(id)sender {
    if (g_dock_cb) g_dock_cb();
}
@end

static DBDockTarget *g_dock_target = nil;

static NSMenu *db_dock_menu_imp(id self, SEL _cmd, NSApplication *sender) {
    (void)self; (void)_cmd; (void)sender;
    NSMenu *menu = [[NSMenu alloc] init];
    NSString *title = g_dock_title.length > 0 ? g_dock_title : @"New Window";
    NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:title
                                                  action:@selector(dbNewWindow:)
                                           keyEquivalent:@""];
    item.target = g_dock_target;
    [menu addItem:item];
    return menu;
}

void db_install_dock_menu(const char *title, db_dock_callback cb) {
    NSString *t = (title && title[0] != '\0')
        ? [NSString stringWithUTF8String:title]
        : nil;
    void (^work)(void) = ^{
        g_dock_cb = cb;
        g_dock_title = t;
        if (g_dock_target == nil) g_dock_target = [[DBDockTarget alloc] init];

        id delegate = [NSApp delegate];
        if (delegate == nil) return;
        Class cls = object_getClass(delegate);
        SEL sel = @selector(applicationDockMenu:);
        // Encoding: returns id, args self (id) + _cmd (SEL) + NSApplication* (id).
        const char *types = "@@:@";
        if (!class_addMethod(cls, sel, (IMP)db_dock_menu_imp, types)) {
            class_replaceMethod(cls, sel, (IMP)db_dock_menu_imp, types);
        }
    };
    if ([NSThread isMainThread]) work();
    else dispatch_sync(dispatch_get_main_queue(), work);
}

#pragma mark - Quick Look

/* Datenquelle für das native QLPreviewPanel. NSURL erfüllt bereits das
 * QLPreviewItem-Protokoll, daher genügt es, die URLs vorzuhalten. */
@interface DBQLDataSource : NSObject <QLPreviewPanelDataSource, QLPreviewPanelDelegate>
@property (strong) NSArray<NSURL *> *urls;
@end

@implementation DBQLDataSource
- (NSInteger)numberOfPreviewItemsInPreviewPanel:(QLPreviewPanel *)panel {
    return (NSInteger)self.urls.count;
}
- (id<QLPreviewItem>)previewPanel:(QLPreviewPanel *)panel previewItemAtIndex:(NSInteger)index {
    if (index < 0 || index >= (NSInteger)self.urls.count) return nil;
    return self.urls[(NSUInteger)index];
}
@end

static DBQLDataSource *g_ql_source = nil;

void db_quick_look(const char *const *paths, int count) {
    if (paths == NULL || count <= 0) return;
    NSMutableArray<NSURL *> *urls = [NSMutableArray arrayWithCapacity:(NSUInteger)count];
    for (int i = 0; i < count; i++) {
        if (paths[i] == NULL) continue;
        NSString *s = [NSString stringWithUTF8String:paths[i]];
        if (s.length == 0) continue;
        [urls addObject:[NSURL fileURLWithPath:s]];
    }
    if (urls.count == 0) return;

    void (^work)(void) = ^{
        if (g_ql_source == nil) g_ql_source = [[DBQLDataSource alloc] init];
        QLPreviewPanel *panel = [QLPreviewPanel sharedPreviewPanel];
        BOOL visible = [QLPreviewPanel sharedPreviewPanelExists] && panel.isVisible;
        // Gleiche Auswahl erneut ausgelöst -> schließen (wie Finder mit Leertaste).
        if (visible && [g_ql_source.urls isEqualToArray:urls]) {
            [panel orderOut:nil];
            return;
        }
        g_ql_source.urls = urls;
        panel.dataSource = g_ql_source;
        panel.delegate = g_ql_source;
        if (!visible) [panel makeKeyAndOrderFront:nil];
        [panel reloadData];
    };
    if ([NSThread isMainThread]) work();
    else dispatch_sync(dispatch_get_main_queue(), work);
}

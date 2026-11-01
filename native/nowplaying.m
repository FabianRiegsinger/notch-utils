// Now Playing bridge for macOS. Loaded into Apple's signed /usr/bin/perl (see nowplaying.pl):
// since macOS 15.4 MediaRemote only answers Apple-entitled processes. Exported functions use the
// Perl XSUB signature (perl interpreter, CV) and ignore both arguments.
//
//   np_stream  — prints one JSON line per now-playing change to stdout, runs forever.
//   np_command — sends the MediaRemote command number in $NP_CMD (2 toggle, 4 next, 5 previous).
#import <Foundation/Foundation.h>
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>

typedef void (*GetInfoFn)(dispatch_queue_t, void (^)(NSDictionary *));
typedef void (*IsPlayingFn)(dispatch_queue_t, void (^)(BOOL));
typedef void (*RegisterFn)(dispatch_queue_t);
typedef BOOL (*SendCommandFn)(int, NSDictionary *);

static void *mr(const char *name) {
    static void *handle;
    if (!handle) handle = dlopen("/System/Library/PrivateFrameworks/MediaRemote.framework/MediaRemote", RTLD_NOW);
    return handle ? dlsym(handle, name) : NULL;
}

static NSData *lastArtwork;

static void emit(NSDictionary *obj) {
    NSData *json = [NSJSONSerialization dataWithJSONObject:obj options:0 error:nil];
    if (!json) return;
    fwrite(json.bytes, 1, json.length, stdout);
    fputc('\n', stdout);
    fflush(stdout);
}

static id str(NSDictionary *info, NSString *key) {
    id v = info[key];
    return [v isKindOfClass:[NSString class]] ? v : @"";
}

static void report(void) {
    GetInfoFn getInfo = (GetInfoFn)mr("MRMediaRemoteGetNowPlayingInfo");
    IsPlayingFn isPlaying = (IsPlayingFn)mr("MRMediaRemoteGetNowPlayingApplicationIsPlaying");
    if (!getInfo || !isPlaying) { emit(@{@"error": @"MediaRemote unavailable"}); return; }
    isPlaying(dispatch_get_main_queue(), ^(BOOL playing) {
        getInfo(dispatch_get_main_queue(), ^(NSDictionary *info) {
            if (info.count == 0) { lastArtwork = nil; emit(@{@"empty": @YES}); return; }
            NSMutableDictionary *o = [NSMutableDictionary dictionary];
            o[@"title"] = str(info, @"kMRMediaRemoteNowPlayingInfoTitle");
            o[@"artist"] = str(info, @"kMRMediaRemoteNowPlayingInfoArtist");
            o[@"album"] = str(info, @"kMRMediaRemoteNowPlayingInfoAlbum");
            o[@"playing"] = @(playing);
            NSNumber *duration = info[@"kMRMediaRemoteNowPlayingInfoDuration"];
            if ([duration isKindOfClass:[NSNumber class]]) o[@"duration"] = duration;
            // Elapsed time is reported as of a timestamp; bring it to "now".
            NSNumber *elapsed = info[@"kMRMediaRemoteNowPlayingInfoElapsedTime"];
            NSNumber *rate = info[@"kMRMediaRemoteNowPlayingInfoPlaybackRate"];
            NSDate *ts = info[@"kMRMediaRemoteNowPlayingInfoTimestamp"];
            double e = [elapsed isKindOfClass:[NSNumber class]] ? elapsed.doubleValue : 0;
            if (playing && [ts isKindOfClass:[NSDate class]]) {
                double r = [rate isKindOfClass:[NSNumber class]] ? rate.doubleValue : 1.0;
                e += -[ts timeIntervalSinceNow] * r;
            }
            o[@"elapsed"] = @(e);
            NSData *art = info[@"kMRMediaRemoteNowPlayingInfoArtworkData"];
            if ([art isKindOfClass:[NSData class]] && art.length > 0) {
                // Only send artwork when it changed: it can be hundreds of KB.
                if (![art isEqualToData:lastArtwork]) {
                    lastArtwork = art;
                    o[@"artwork"] = [art base64EncodedStringWithOptions:0];
                }
            } else if (lastArtwork) {
                lastArtwork = nil;
                o[@"artwork"] = @"";
            }
            emit(o);
        });
    });
}

void np_stream(void *perl, void *cv) {
    (void)perl; (void)cv;
    RegisterFn reg = (RegisterFn)mr("MRMediaRemoteRegisterForNowPlayingNotifications");
    if (!reg) { emit(@{@"error": @"MediaRemote unavailable"}); return; }
    reg(dispatch_get_main_queue());
    NSNotificationCenter *nc = [NSNotificationCenter defaultCenter];
    for (NSString *name in @[@"kMRMediaRemoteNowPlayingInfoDidChangeNotification",
                             @"kMRMediaRemoteNowPlayingApplicationIsPlayingDidChangeNotification",
                             @"kMRMediaRemoteNowPlayingApplicationDidChangeNotification"]) {
        [nc addObserverForName:name object:nil queue:nil usingBlock:^(NSNotification *n) { (void)n; report(); }];
    }
    report();
    // Exit when the parent (notch-utils) goes away: stdin closes.
    dispatch_source_t src = dispatch_source_create(DISPATCH_SOURCE_TYPE_READ, 0, 0, dispatch_get_main_queue());
    dispatch_source_set_event_handler(src, ^{
        char buf[64];
        if (read(0, buf, sizeof buf) <= 0) exit(0);
    });
    dispatch_resume(src);
    CFRunLoopRun();
}

void np_command(void *perl, void *cv) {
    (void)perl; (void)cv;
    SendCommandFn send = (SendCommandFn)mr("MRMediaRemoteSendCommand");
    const char *c = getenv("NP_CMD");
    if (send && c) send(atoi(c), nil);
}

/**
 * AU v2 Cocoa UI view factory.
 *
 * Defines the `AUCocoaUIBase` class the host instantiates after
 * reading our `kAudioUnitProperty_CocoaUI`. The class is compiled
 * into every truce plugin dylib so it appears in `__objc_classlist`,
 * which `[NSBundle classNamed:]`-based hosts (REAPER) require.
 *
 * The class name MUST be unique per plugin. AppKit/AudioUnit hosts
 * load every installed `.component` into one process; if two plugins
 * publish a class with the same name, `libobjc` keeps the first one
 * and `[NSBundle classNamed:name]` on the loser's bundle returns nil
 * - the host then thinks the plugin has no GUI. Uniqueness comes
 * from the `TRUCE_AU_PLUGIN_ID` env var that `cargo-truce` sets at
 * build time; the build.rs sanitises and passes it as a `-D` define.
 * Without that env (plain `cargo build` for unit tests), the class
 * falls back to a default name - fine for isolated tests, not for
 * multi-plugin hosting.
 */

@import AppKit;
@import AudioToolbox;
#import <AudioUnit/AUCocoaUIView.h>

#include "au_shim_types.h"

// Private properties exposed by `au_v2_shim.c`:
//   64000: AuPlugin context pointer (rustCtx)
//   64001: pointer to the AU's AuCallbacks table (g_callbacks of the
//          dylib that owns this AudioUnit). Reading both via the AU
//          dispatch table keeps the methods plugin-agnostic - per-
//          dylib globals reached are always the right ones.
#define kTrucePrivateProperty_RustContext  64000
#define kTrucePrivateProperty_AuCallbacks  64001

#ifndef TRUCE_AU_VIEW_FACTORY_NAME
// Default name when `TRUCE_AU_PLUGIN_ID` is unset - keeps `cargo build`
// of the workspace cdylibs working for unit tests.
#define TRUCE_AU_VIEW_FACTORY_NAME TruceAUCocoaViewProxy
#endif
#ifndef TRUCE_AU_FIXED_CONTAINER_NAME
#define TRUCE_AU_FIXED_CONTAINER_NAME TruceAuFixedContainer
#endif

/// Container the host parents the editor into.
///
/// DuckDSP patch (Sonder, 2026-10-26, see vendor/truce-au/PATCHES.md).
/// Upstream truce 6.3.0 pins this view to the editor's natural size and
/// ignores every resize, so in Logic a plugin-driven `request_resize`
/// only grows the editor's child view inside a frame that never moves,
/// and the window edges do nothing. Here:
/// - the container follows the editor's own view: when the editor
///   resizes itself (zoom buttons, in-plugin grip), the container takes
///   the new size, and the host, which observes its plug-in view's frame,
///   resizes its window;
/// - a host resize goes to the editor through `gui_set_size`, which
///   clamps to min/max/aspect, and the container adopts the clamped size;
/// - editors that are not resizable keep the upstream fixed behaviour.
///
/// Logic hosts AUv2 plug-ins out of process (AUHostingServiceXPC) and
/// relays the view: it only sees our superview inside the service window,
/// not our own frame. So a plugin-driven resize resizes the service
/// window once (`setContentSize:`) and lets the autoresizing chain bring
/// the superview and us along (JUCE's AUv2 wrapper also resizes its
/// superview). Logic's service window is not resizable (no
/// `NSWindowStyleMaskResizable`), so its edges never reach us: host-driven
/// resizing stays inactive in Logic.
@interface TRUCE_AU_FIXED_CONTAINER_NAME : NSView
@property(nonatomic, assign) void *rustCtx;
@property(nonatomic, assign) const AuCallbacks *callbacks;
/// Read once before `gui_open`. `gui_can_resize` takes the editor cell
/// with a plain `enter()`, so it must not be called from a frame
/// notification that may fire while `gui_open` holds that cell.
@property(nonatomic, assign) BOOL resizable;
/// Set while we resize the window ourselves, so the autoresize it
/// triggers on us is not taken for a host-driven resize.
@property(nonatomic, assign) BOOL pluginResizing;
@end

@implementation TRUCE_AU_FIXED_CONTAINER_NAME

- (BOOL)truceEditorResizable {
    return self.resizable && self.rustCtx != NULL && self.callbacks != NULL;
}

- (NSSize)truceEditorSize {
    uint32_t w = 0, h = 0;
    if (self.rustCtx != NULL && self.callbacks != NULL) {
        self.callbacks->gui_get_size(self.rustCtx, &w, &h);
    }
    return NSMakeSize((CGFloat)w, (CGFloat)h);
}

- (void)setFrameSize:(NSSize)newSize {
    if (self.rustCtx == NULL || self.callbacks == NULL) {
        [super setFrameSize:newSize];
        return;
    }
    if (!self.pluginResizing && [self truceEditorResizable] && newSize.width >= 1.0 &&
        newSize.height >= 1.0) {
        // Host-driven resize: the editor clamps it, we adopt the result.
        self.callbacks->gui_set_size(self.rustCtx, (uint32_t)lround(newSize.width),
                                     (uint32_t)lround(newSize.height));
    }
    NSSize s = [self truceEditorSize];
    [super setFrameSize:(s.width > 0 && s.height > 0) ? s : newSize];
}

- (void)didAddSubview:(NSView *)subview {
    [super didAddSubview:subview];
    subview.postsFrameChangedNotifications = YES;
    [[NSNotificationCenter defaultCenter] addObserver:self
                                             selector:@selector(truceEditorFrameChanged:)
                                                 name:NSViewFrameDidChangeNotification
                                               object:subview];
}

- (void)willRemoveSubview:(NSView *)subview {
    [[NSNotificationCenter defaultCenter] removeObserver:self
                                                    name:NSViewFrameDidChangeNotification
                                                  object:subview];
    [super willRemoveSubview:subview];
}

/// The editor resized its own view (plugin-driven resize): follow it so
/// the host sees the plug-in view change size.
- (void)truceEditorFrameChanged:(NSNotification *)note {
    if (![self truceEditorResizable]) {
        return;
    }
    NSView *editorView = note.object;
    NSSize s = editorView.frame.size;
    if (s.width < 1.0 || s.height < 1.0 || NSEqualSizes(s, self.frame.size)) {
        return;
    }
    NSSize old = self.frame.size;
    NSView *host = self.superview;
    self.pluginResizing = YES;
    NSWindow *win = self.window;
    if (win != nil && host != nil && NSEqualSizes(host.frame.size, old)) {
        // One resize, from the top: the window takes the new content size
        // and the autoresizing chain (content view -> superview -> us)
        // follows by the same delta. Resizing the superview directly made
        // Logic's service resize the window too, and the superview then
        // got the delta twice (320 -> 160 -> 240).
        NSSize cs = win.contentView.frame.size;
        [win setContentSize:NSMakeSize(cs.width + s.width - old.width,
                                       cs.height + s.height - old.height)];
    } else if (host != nil) {
        NSSize hs = host.frame.size;
        [host setFrameSize:NSMakeSize(hs.width + s.width - old.width,
                                      hs.height + s.height - old.height)];
    }
    if (!NSEqualSizes(self.frame.size, s)) {
        [super setFrameSize:s];
    }
    self.pluginResizing = NO;
}

- (void)dealloc {
    [[NSNotificationCenter defaultCenter] removeObserver:self];
}
@end

@interface TRUCE_AU_VIEW_FACTORY_NAME : NSObject <AUCocoaUIBase>
@end

@implementation TRUCE_AU_VIEW_FACTORY_NAME

- (unsigned)interfaceVersion {
    return 0;
}

- (NSView *)uiViewForAudioUnit:(AudioUnit)au withSize:(NSSize)preferredSize {
    void *ctx = NULL;
    UInt32 ctxSize = sizeof(ctx);
    if (AudioUnitGetProperty(au, kTrucePrivateProperty_RustContext,
            kAudioUnitScope_Global, 0, &ctx, &ctxSize) != noErr || !ctx) {
        return nil;
    }

    const AuCallbacks *cb = NULL;
    UInt32 cbSize = sizeof(cb);
    if (AudioUnitGetProperty(au, kTrucePrivateProperty_AuCallbacks,
            kAudioUnitScope_Global, 0, &cb, &cbSize) != noErr || !cb) {
        return nil;
    }

    if (!cb->gui_has_editor(ctx)) return nil;

    // `preferredSize` is ignored: the view opens at the editor's
    // size. Resizing afterwards is handled by the container (DuckDSP
    // patch above).
    uint32_t w = 0, h = 0;
    cb->gui_get_size(ctx, &w, &h);
    if (w == 0 || h == 0) return nil;
    (void)preferredSize;

    NSRect frame = NSMakeRect(0, 0, w, h);
    TRUCE_AU_FIXED_CONTAINER_NAME *container =
        [[TRUCE_AU_FIXED_CONTAINER_NAME alloc] initWithFrame:frame];
    container.rustCtx = ctx;
    container.callbacks = cb;
    container.resizable = cb->gui_can_resize(ctx) != 0;
    if (container.resizable) {
        container.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
    }
    cb->gui_open(ctx, (__bridge void *)container);
    return container;
}

@end

// Stringify the class name for the v2 shim's `kAudioUnitProperty_CocoaUI`
// response. Two-step macro so the argument is expanded before stringification.
#define _TRUCE_STRINGIFY(x) #x
#define TRUCE_STRINGIFY(x) _TRUCE_STRINGIFY(x)

const char *truce_au_view_factory_class_name(void) {
    return TRUCE_STRINGIFY(TRUCE_AU_VIEW_FACTORY_NAME);
}

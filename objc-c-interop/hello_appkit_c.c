#include <stdio.h>
#include <stdlib.h>
#include <stdbool.h>

#include <objc/objc.h>
#include <objc/runtime.h>
#include <objc/message.h>

typedef double CGFloat;

typedef struct {
    CGFloat x;
    CGFloat y;
} NSPoint;

typedef struct {
    CGFloat width;
    CGFloat height;
} NSSize;

typedef struct {
    NSPoint origin;
    NSSize size;
} NSRect;

static id ns_string(const char *utf8) {
    Class NSString = objc_getClass("NSString");
    SEL sel = sel_registerName("stringWithUTF8String:");

    return ((id (*)(id, SEL, const char *))objc_msgSend)(
        (id)NSString,
        sel,
        utf8
    );
}

static signed char app_should_terminate_after_last_window_closed(
    id self,
    SEL _cmd,
    id application
) {
    return 1;
}

int main(void) {
    // Create autorelease pool
    Class PoolClass = objc_getClass("NSAutoreleasePool");
    id pool = ((id (*)(id, SEL))objc_msgSend)(
        (id)PoolClass,
        sel_registerName("alloc")
    );

    pool = ((id (*)(id, SEL))objc_msgSend)(
        pool,
        sel_registerName("init")
    );

    // NSApplication.sharedApplication
    Class NSApplication = objc_getClass("NSApplication");
    id app = ((id (*)(id, SEL))objc_msgSend)(
        (id)NSApplication,
        sel_registerName("sharedApplication")
    );

    // app.activationPolicy = regular
    // NSApplicationActivationPolicyRegular == 0
    ((void (*)(id, SEL, long))objc_msgSend)(
        app,
        sel_registerName("setActivationPolicy:"),
        0
    );

    // Create a tiny app delegate so closing the window quits the app
    Class NSObject = objc_getClass("NSObject");

    Class AppDelegate = objc_allocateClassPair(
        NSObject,
        "CAppDelegate",
        0
    );

    class_addMethod(
        AppDelegate,
        sel_registerName("applicationShouldTerminateAfterLastWindowClosed:"),
        (IMP)app_should_terminate_after_last_window_closed,
        "c@:@"
    );

    objc_registerClassPair(AppDelegate);

    id delegate = ((id (*)(id, SEL))objc_msgSend)(
        (id)AppDelegate,
        sel_registerName("new")
    );

    ((void (*)(id, SEL, id))objc_msgSend)(
        app,
        sel_registerName("setDelegate:"),
        delegate
    );

    // Window constants
    unsigned long NSWindowStyleMaskTitled        = 1 << 0;
    unsigned long NSWindowStyleMaskClosable      = 1 << 1;
    unsigned long NSWindowStyleMaskMiniaturizable = 1 << 2;
    unsigned long NSWindowStyleMaskResizable     = 1 << 3;

    unsigned long style =
        NSWindowStyleMaskTitled |
        NSWindowStyleMaskClosable |
        NSWindowStyleMaskMiniaturizable |
        NSWindowStyleMaskResizable;

    // NSBackingStoreBuffered == 2
    unsigned long backing = 2;

    NSRect frame = {
        .origin = { .x = 0, .y = 0 },
        .size = { .width = 800, .height = 500 }
    };

    // Create NSWindow
    Class NSWindow = objc_getClass("NSWindow");

    id window = ((id (*)(id, SEL))objc_msgSend)(
        (id)NSWindow,
        sel_registerName("alloc")
    );

    window = ((id (*)(id, SEL, NSRect, unsigned long, unsigned long, signed char))objc_msgSend)(
        window,
        sel_registerName("initWithContentRect:styleMask:backing:defer:"),
        frame,
        style,
        backing,
        0
    );

    ((void (*)(id, SEL, id))objc_msgSend)(
        window,
        sel_registerName("setTitle:"),
        ns_string("Hello from C + Objective-C Runtime")
    );

    ((void (*)(id, SEL))objc_msgSend)(
        window,
        sel_registerName("center")
    );

    // Create NSTextField label
    Class NSTextField = objc_getClass("NSTextField");

    id label = ((id (*)(id, SEL))objc_msgSend)(
        (id)NSTextField,
        sel_registerName("alloc")
    );

    label = ((id (*)(id, SEL, NSRect))objc_msgSend)(
        label,
        sel_registerName("initWithFrame:"),
        frame
    );

    ((void (*)(id, SEL, id))objc_msgSend)(
        label,
        sel_registerName("setStringValue:"),
        ns_string("Hello world")
    );

    // NSTextAlignmentCenter == 2
    ((void (*)(id, SEL, long))objc_msgSend)(
        label,
        sel_registerName("setAlignment:"),
        2
    );

    // Make it look like a plain label
    ((void (*)(id, SEL, signed char))objc_msgSend)(
        label,
        sel_registerName("setBezeled:"),
        0
    );

    ((void (*)(id, SEL, signed char))objc_msgSend)(
        label,
        sel_registerName("setDrawsBackground:"),
        0
    );

    ((void (*)(id, SEL, signed char))objc_msgSend)(
        label,
        sel_registerName("setEditable:"),
        0
    );

    ((void (*)(id, SEL, signed char))objc_msgSend)(
        label,
        sel_registerName("setSelectable:"),
        0
    );

    // Use a bigger font
    Class NSFont = objc_getClass("NSFont");

    id font = ((id (*)(id, SEL, CGFloat))objc_msgSend)(
        (id)NSFont,
        sel_registerName("boldSystemFontOfSize:"),
        36.0
    );

    ((void (*)(id, SEL, id))objc_msgSend)(
        label,
        sel_registerName("setFont:"),
        font
    );

    // Resize label with the window
    // NSViewWidthSizable == 2, NSViewHeightSizable == 16
    ((void (*)(id, SEL, unsigned long))objc_msgSend)(
        label,
        sel_registerName("setAutoresizingMask:"),
        2 | 16
    );

    // Add label to window content view
    id contentView = ((id (*)(id, SEL))objc_msgSend)(
        window,
        sel_registerName("contentView")
    );

    ((void (*)(id, SEL, id))objc_msgSend)(
        contentView,
        sel_registerName("addSubview:"),
        label
    );

    // Show window
    ((void (*)(id, SEL, id))objc_msgSend)(
        window,
        sel_registerName("makeKeyAndOrderFront:"),
        NULL
    );

    // Bring app to front
    ((void (*)(id, SEL, signed char))objc_msgSend)(
        app,
        sel_registerName("activateIgnoringOtherApps:"),
        1
    );

    // Run app event loop
    ((void (*)(id, SEL))objc_msgSend)(
        app,
        sel_registerName("run")
    );

    // Drain pool after app exits
    ((void (*)(id, SEL))objc_msgSend)(
        pool,
        sel_registerName("drain")
    );

    return 0;
}
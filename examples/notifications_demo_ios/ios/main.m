// The bundle's entry point: `main` hands the process to C+, and
// notifications_demo_ios_main never comes back — it calls UIApplicationMain,
// which owns the process from there.
#import "notifications_demo_ios.h"

int main(int argc, char *argv[]) {
    return notifications_demo_ios_main();
}

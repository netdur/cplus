// The runner's C entry point. No UIApplicationMain: the one check that needs
// the main run loop turns it itself with a deadline, and a process that never
// exits cannot report a failure count.
#import "permissions_tests.h"

int main(int argc, char *argv[]) {
    return permissions_tests_main();
}

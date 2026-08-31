// The runner's C entry point. No UIApplicationMain: the checks need the
// framework and a bundle, not a run loop.
#import "notifications_tests.h"

int main(int argc, char *argv[]) {
    return notifications_tests_main();
}

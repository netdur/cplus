// The runner's C entry point. No UIApplicationMain: the checks need the
// keychain, not a run loop, and a process that never exits cannot report.
#import "securestore_tests.h"

int main(int argc, char *argv[]) {
    return securestore_tests_main();
}

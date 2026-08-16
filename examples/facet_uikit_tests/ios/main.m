// The runner's C entry point. No UIApplicationMain: the checks need the
// framework, not a run loop, and a process that never exits cannot report.
#import "facet_uikit_tests.h"

int main(int argc, char *argv[]) {
    return facet_uikit_tests_main();
}

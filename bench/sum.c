#include <stdio.h>
#include <stdint.h>

int main(void) {
    int64_t sum = 0;
    int64_t n = 1000000000;
    for (int64_t i = 1; i <= n; i++) {
        sum += i;
    }
    printf("%d\n", (int)sum);
    return 0;
}

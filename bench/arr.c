#include <stdio.h>
#include <stdint.h>

int main(void) {
    int32_t a[8] = {1, 2, 3, 4, 5, 6, 7, 8};
    int64_t sum = 0;
    for (int32_t iter = 0; iter < 100000000; iter++) {
        size_t idx = (size_t)((int64_t)iter % 8);
        sum += (int64_t)a[idx];
    }
    printf("%d\n", (int)sum);
    return 0;
}

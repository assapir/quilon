#include <stdio.h>

extern double quilon_main();

int main() {
    double result = quilon_main();
    printf("Result: %f\n", result);
    return 0;
}

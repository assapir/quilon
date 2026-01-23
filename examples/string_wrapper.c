#include <stdio.h>
#include <string.h>

extern char* quilon_main();

int main() {
    char* result = quilon_main();
    printf("Result: %s\n", result);
    return 0;
}

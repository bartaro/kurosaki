#include "lib/fc.h"

unsigned char smoke_counter;

void main(void)
{
    nes_ppu_screen_on(0x80, 0x1e);
    smoke_counter = 0;
    while (1)
    {
        smoke_counter = smoke_counter + 1;
        __vramq_put(0x2000, smoke_counter);
        __nmi_wait();
    }
}

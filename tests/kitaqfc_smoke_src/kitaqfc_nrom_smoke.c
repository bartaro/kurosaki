#include "lib/fc.h"

// One byte of observable loop state for the compiler/emulator smoke fixture.
unsigned char smoke_counter;

// Enable rendering and repeatedly queue one nametable-byte update, yielding
// to the NMI wait after each iteration. This minimal fixture exercises the
// PPU/queue path rather than presenting a complete game scene.
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

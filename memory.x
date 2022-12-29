MEMORY
{
    FLASH : ORIGIN = 0x08000000, LENGTH = 1024K
    RAM : ORIGIN = 0x20000000, LENGTH = 96K
    /* Separate bank of memory, for now 96K from the main bank is enough */
    /* RAM2 : ORIGIN = 0x10000000, LENGTH = 32K */
}

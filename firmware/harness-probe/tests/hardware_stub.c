#include <assert.h>
#include <string.h>
#include "hardware_stub.h"
#include "pico_dualcore_config.h"
#include "util.h"
static struct mock_io regs;
struct mock_io *io_bank0_hw = &regs;
unsigned levels[30], directions[30], overrides[30], pulls[30];
unsigned adc_value, adc_channel, now_us, ina_failure, power_read_bytes, off_was_hiz;
uint32_t ina_registers[64];
static uint8_t selected_register;
static mock_spi_hw spi_regs;
static int next_dma_channel;
static int next_lock;
static spin_lock_t mock_locks[4];
static mock_uart_hw uart_regs;
static dma_channel_hw_t mock_dma_channels[8];
static uint32_t mock_clocks[2];
static pio_hw_t pio0_hw;
pio_hw_t *pio0 = &pio0_hw;

void mock_reset(void)
{
    memset(&regs, 0, sizeof(regs));
    memset(levels, 0, sizeof(levels));
    memset(directions, 0, sizeof(directions));
    memset(overrides, 0, sizeof(overrides));
    memset(pulls, 1, sizeof(pulls));
    memset(ina_registers, 0, sizeof(ina_registers));
    ina_registers[0x3e] = 0x5449;
    levels[1] = levels[2] = 1;
    adc_value = now_us = ina_failure = power_read_bytes = off_was_hiz = 0;
    next_dma_channel = 0;
    next_lock = 0;
    mock_clocks[clk_sys] = mock_clocks[clk_peri] = 125000000;
}
void critical_section_init(critical_section_t *lock)
{
    *lock = 0;
}
void critical_section_enter_blocking(critical_section_t *lock)
{
    assert(!*lock);
    *lock = 1;
}
void critical_section_exit(critical_section_t *lock)
{
    assert(*lock);
    *lock = 0;
}
void hw_write_masked(volatile uint32_t *reg, uint32_t value, uint32_t mask)
{
    *reg = (*reg & ~mask) | (value & mask);
}
void gpio_init(uint pin)
{
    directions[pin] = 0;
    regs.io[pin].ctrl = GPIO_FUNC_SIO;
    overrides[pin] = 0;
}
void gpio_disable_pulls(uint pin)
{
    pulls[pin] = 0;
}
void gpio_put(uint pin, bool value)
{
    if (pin == 0 && !value && levels[0]) {
        off_was_hiz = 1;
        for (uint p = 0; p < 30; ++p)
            if ((0x083fffc8u & (1u << p)) && overrides[p] != GPIO_OVERRIDE_LOW)
                off_was_hiz = 0;
    }
    levels[pin] = value;
}
void gpio_set_dir(uint pin, bool output)
{
    directions[pin] = output;
}
void gpio_set_oeover(uint pin, uint override)
{
    overrides[pin] = override;
}
void gpio_set_function(uint pin, gpio_function_t function)
{
    regs.io[pin].ctrl = function;
    overrides[pin] = 0;
}
bool gpio_get(uint pin)
{
    return levels[pin];
}
void adc_init(void)
{
}
void adc_gpio_init(uint pin)
{
    pulls[pin] = 0;
    directions[pin] = 0;
}
void adc_select_input(uint channel)
{
    adc_channel = channel;
}
uint16_t adc_read(void)
{
    assert(adc_channel == 2);
    return adc_value;
}
void i2c_init(int port, uint baud)
{
    assert(port == 0 && baud == 400000);
}
int i2c_write_timeout_us(int port, uint8_t address, const uint8_t *bytes, size_t count, bool nostop,
                         uint timeout)
{
    assert(port == 0 && address == 0x40 && timeout == 5000);
    if (ina_failure)
        return -1;
    selected_register = bytes[0];
    if (count == 3) {
        assert(!nostop);
        ina_registers[bytes[0]] = (bytes[1] << 8) | bytes[2];
    } else
        assert(count == 1 && nostop);
    return count;
}
int i2c_read_timeout_us(int port, uint8_t address, uint8_t *bytes, size_t count, bool nostop,
                        uint timeout)
{
    assert(port == 0 && address == 0x40 && !nostop && timeout == 5000);
    if (ina_failure)
        return -1;
    if (selected_register == 8)
        power_read_bytes = count;
    uint32_t value = ina_registers[selected_register];
    for (size_t i = 0; i < count; ++i)
        bytes[i] = value >> (8 * (count - 1 - i));
    return count;
}
uint32_t time_us_32(void)
{
    return now_us;
}
void sleep_ms(uint milliseconds)
{
    now_us += milliseconds * 1000;
}
void busy_wait_ms(uint milliseconds)
{
    now_us += milliseconds * 1000;
}
bool spi_is_readable(int port)
{
    assert(port == spi1);
    return false;
}
bool spi_is_busy(int port)
{
    assert(port == spi1);
    return false;
}
mock_spi_hw *spi_get_hw(int port)
{
    assert(port == spi1);
    return &spi_regs;
}
int spi_get_dreq(int port, bool is_tx)
{
    assert(port == spi1);
    return is_tx;
}
uint spi_init(int port, uint baud)
{
    assert(port == spi1);
    return baud;
}
void spi_set_format(int port, uint bits, int cpol, int cpha, int order)
{
    assert(port == spi1 && bits == 8 && cpol == SPI_CPOL_0 && cpha == SPI_CPHA_0 &&
           order == SPI_MSB_FIRST);
}
int dma_claim_unused_channel(bool required)
{
    assert(required);
    return next_dma_channel++;
}
dma_channel_config dma_channel_get_default_config(int channel)
{
    return channel;
}
void channel_config_set_transfer_data_size(dma_channel_config *config, int size)
{
    (void)config;
    assert(size == DMA_SIZE_8);
}
void channel_config_set_read_increment(dma_channel_config *config, bool increment)
{
    (void)config;
    (void)increment;
}
void channel_config_set_write_increment(dma_channel_config *config, bool increment)
{
    (void)config;
    (void)increment;
}
void channel_config_set_dreq(dma_channel_config *config, int dreq)
{
    (void)config;
    (void)dreq;
}
void dma_channel_configure(int channel, dma_channel_config *config, volatile void *write,
                           const volatile void *read, uint count, bool trigger)
{
    (void)channel;
    (void)config;
    (void)write;
    (void)read;
    (void)count;
    (void)trigger;
}
void dma_start_channel_mask(uint32_t mask)
{
    (void)mask;
}
bool dma_channel_is_busy(int channel)
{
    (void)channel;
    return false;
}
void dma_channel_abort(int channel)
{
    (void)channel;
}
void channel_config_set_ring(dma_channel_config *config, bool ring, uint size_bits)
{
    (void)config;
    (void)ring;
    (void)size_bits;
}
dma_channel_hw_t *dma_channel_hw_addr(int channel)
{
    return &mock_dma_channels[channel];
}
uint32_t clock_get_hz(enum clock_index clk)
{
    return mock_clocks[clk];
}
int spin_lock_claim_unused(bool required)
{
    assert(required);
    return next_lock++;
}
spin_lock_t *spin_lock_instance(int lock_num)
{
    return &mock_locks[lock_num];
}
uint32_t spin_lock_blocking(spin_lock_t *lock)
{
    (void)lock;
    return 0;
}
void spin_unlock(spin_lock_t *lock, uint32_t irq)
{
    (void)lock;
    (void)irq;
}
void pico_get_unique_board_id(pico_unique_board_id_t *id_out)
{
    memset(id_out, 0, sizeof(*id_out));
}
TickType_t xTaskGetTickCount(void)
{
    return 0;
}
bool tud_ready(void)
{
    return true;
}
mock_uart_hw *uart_get_hw(int port)
{
    assert(port == uart1);
    return &uart_regs;
}
int uart_get_dreq(int port, bool is_tx)
{
    assert(port == uart1);
    return is_tx;
}
uint uart_init(int port, uint baud)
{
    assert(port == uart1);
    return baud;
}
void uart_set_hw_flow(int port, bool cts, bool rts)
{
    (void)port;
    (void)cts;
    (void)rts;
}
void uart_set_fifo_enabled(int port, bool enabled)
{
    (void)port;
    (void)enabled;
}
uint uart_set_baudrate(int port, uint baud)
{
    assert(port == uart1);
    return baud;
}
void uart_set_format(int port, uint bits, uint stop, int parity)
{
    (void)port;
    (void)bits;
    (void)stop;
    (void)parity;
}
bool uart_is_readable(int port)
{
    (void)port;
    return false;
}
char uart_getc(int port)
{
    (void)port;
    return 0;
}
uint pio_add_program(pio_hw_t *pio, const pio_program_t *program)
{
    (void)pio;
    (void)program;
    return 0;
}
void pio_remove_program(pio_hw_t *pio, const pio_program_t *program, uint loaded_offset)
{
    (void)pio;
    (void)program;
    (void)loaded_offset;
}
void pio_sm_put_blocking(pio_hw_t *pio, uint sm, uint32_t data)
{
    (void)pio;
    (void)sm;
    (void)data;
}
uint32_t pio_sm_get_blocking(pio_hw_t *pio, uint sm)
{
    (void)pio;
    (void)sm;
    return 0;
}
void pio_sm_exec(pio_hw_t *pio, uint sm, uint instr)
{
    (void)pio;
    (void)sm;
    (void)instr;
}
void pio_sm_init(pio_hw_t *pio, uint sm, uint offset, const pio_sm_config *config)
{
    (void)pio;
    (void)sm;
    (void)offset;
    (void)config;
}
void pio_sm_set_enabled(pio_hw_t *pio, uint sm, bool enabled)
{
    (void)pio;
    (void)sm;
    (void)enabled;
}
void pio_sm_set_consecutive_pindirs(pio_hw_t *pio, uint sm, uint pin, uint count, bool is_out)
{
    (void)pio;
    (void)sm;
    (void)pin;
    (void)count;
    (void)is_out;
}
void pio_sm_set_clkdiv_int_frac(pio_hw_t *pio, uint sm, uint16_t div_int, uint8_t div_frac)
{
    (void)pio;
    (void)sm;
    (void)div_int;
    (void)div_frac;
}
void sm_config_set_sideset_pins(pio_sm_config *config, uint sideset_base)
{
    (void)config;
    (void)sideset_base;
}
void sm_config_set_out_pins(pio_sm_config *config, uint out_base, uint out_count)
{
    (void)config;
    (void)out_base;
    (void)out_count;
}
void sm_config_set_set_pins(pio_sm_config *config, uint set_base, uint set_count)
{
    (void)config;
    (void)set_base;
    (void)set_count;
}
void sm_config_set_in_pins(pio_sm_config *config, uint in_base)
{
    (void)config;
    (void)in_base;
}
void sm_config_set_out_shift(pio_sm_config *config, bool shift_right, bool autopull,
                             uint pull_threshold)
{
    (void)config;
    (void)shift_right;
    (void)autopull;
    (void)pull_threshold;
}
void sm_config_set_in_shift(pio_sm_config *config, bool shift_right, bool autopush,
                            uint push_threshold)
{
    (void)config;
    (void)shift_right;
    (void)autopush;
    (void)push_threshold;
}
uint32_t mock_normalize(uint32_t sample)
{
    return normalize_gpio(sample);
}

const char *mock_info(void)
{
    return PDBG_INFO_STRING;
}

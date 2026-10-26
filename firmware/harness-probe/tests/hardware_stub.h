/* Host-only hardware model used to execute ph_fw_target_control.c. */
#ifndef PH_FW_HARDWARE_STUB_H
#define PH_FW_HARDWARE_STUB_H
#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>
typedef unsigned uint;
typedef unsigned gpio_function_t;
typedef int critical_section_t;
#define count_of(x) (sizeof(x) / sizeof((x)[0]))
#define GPIO_OUT 1
#define GPIO_IN 0
#define GPIO_FUNC_SIO 5
#define GPIO_FUNC_I2C 3
#define GPIO_FUNC_PIO0 6
#define GPIO_FUNC_SPI 1
#define GPIO_FUNC_UART 2
#define GPIO_OVERRIDE_LOW 2
#define GPIO_OVERRIDE_NORMAL 0
#define IO_BANK0_GPIO0_CTRL_FUNCSEL_LSB 0
#define IO_BANK0_GPIO0_CTRL_FUNCSEL_BITS 0x1fu
#define i2c0 0
#define spi1 1
#define DMA_SIZE_8 1
#define SPI_CPOL_0 0
#define SPI_CPHA_0 0
#define SPI_MSB_FIRST 0
typedef int dma_channel_config;
typedef struct {
    volatile uint32_t dr;
} mock_spi_hw;
struct mock_io {
    struct {
        volatile uint32_t ctrl;
    } io[30];
};
extern struct mock_io *io_bank0_hw;
void critical_section_init(critical_section_t *lock);
void critical_section_enter_blocking(critical_section_t *lock);
void critical_section_exit(critical_section_t *lock);
void hw_write_masked(volatile uint32_t *reg, uint32_t value, uint32_t mask);
void gpio_init(uint pin);
void gpio_disable_pulls(uint pin);
void gpio_put(uint pin, bool value);
void gpio_set_dir(uint pin, bool output);
void gpio_set_oeover(uint pin, uint override);
void gpio_set_function(uint pin, gpio_function_t function);
bool gpio_get(uint pin);
void adc_init(void);
void adc_gpio_init(uint pin);
void adc_select_input(uint channel);
uint16_t adc_read(void);
void i2c_init(int port, uint baud);
int i2c_write_timeout_us(int port, uint8_t address, const uint8_t *bytes, size_t count, bool nostop,
                         uint timeout);
int i2c_read_timeout_us(int port, uint8_t address, uint8_t *bytes, size_t count, bool nostop,
                        uint timeout);
uint32_t time_us_32(void);
void sleep_ms(uint milliseconds);
void busy_wait_ms(uint milliseconds);
bool spi_is_readable(int port);
bool spi_is_busy(int port);
mock_spi_hw *spi_get_hw(int port);
int spi_get_dreq(int port, bool is_tx);
uint spi_init(int port, uint baud);
void spi_set_format(int port, uint bits, int cpol, int cpha, int order);
int dma_claim_unused_channel(bool required);
dma_channel_config dma_channel_get_default_config(int channel);
void channel_config_set_transfer_data_size(dma_channel_config *config, int size);
void channel_config_set_read_increment(dma_channel_config *config, bool increment);
void channel_config_set_write_increment(dma_channel_config *config, bool increment);
void channel_config_set_dreq(dma_channel_config *config, int dreq);
void dma_channel_configure(int channel, dma_channel_config *config, volatile void *write,
                           const volatile void *read, uint count, bool trigger);
void dma_start_channel_mask(uint32_t mask);
bool dma_channel_is_busy(int channel);
void dma_channel_abort(int channel);
/* ---- ph_fw_target_io.c: UART/DMA ring engine ---- */
#define uart1 1
#define UART_PARITY_NONE 0
#define UART_PARITY_ODD 1
#define UART_PARITY_EVEN 2
typedef struct {
    volatile uint32_t dr;
    volatile uint32_t rsr;
} mock_uart_hw;
mock_uart_hw *uart_get_hw(int port);
int uart_get_dreq(int port, bool is_tx);
uint uart_init(int port, uint baud);
void uart_set_hw_flow(int port, bool cts, bool rts);
void uart_set_fifo_enabled(int port, bool enabled);
uint uart_set_baudrate(int port, uint baud);
void uart_set_format(int port, uint bits, uint stop, int parity);
bool uart_is_readable(int port);
char uart_getc(int port);
void channel_config_set_ring(dma_channel_config *config, bool ring, uint size_bits);
typedef struct {
    volatile uint32_t transfer_count;
} dma_channel_hw_t;
dma_channel_hw_t *dma_channel_hw_addr(int channel);
/* ---- ph_fw_target_io.c: clocks / spin locks ---- */
enum clock_index { clk_sys, clk_peri };
uint32_t clock_get_hz(enum clock_index clk);
typedef int spin_lock_t;
int spin_lock_claim_unused(bool required);
spin_lock_t *spin_lock_instance(int lock_num);
uint32_t spin_lock_blocking(spin_lock_t *lock);
void spin_unlock(spin_lock_t *lock, uint32_t irq);
/* ---- ph_fw_target_io.c: unique id ---- */
#define PICO_UNIQUE_BOARD_ID_SIZE_BYTES 8
typedef struct {
    uint8_t id[PICO_UNIQUE_BOARD_ID_SIZE_BYTES];
} pico_unique_board_id_t;
void pico_get_unique_board_id(pico_unique_board_id_t *id_out);
/* ---- ph_fw_target_io.c: SWD PIO engine ---- */
#define PIO_FDEBUG_TXSTALL_LSB 9
#define PIO_FDEBUG_RXSTALL_LSB 24
typedef struct {
    const uint16_t *instructions;
    uint8_t length;
    int8_t origin;
} pio_program_t;
typedef int pio_sm_config;
typedef struct {
    volatile uint32_t fdebug;
} pio_hw_t;
extern pio_hw_t *pio0;
uint pio_add_program(pio_hw_t *pio, const pio_program_t *program);
void pio_remove_program(pio_hw_t *pio, const pio_program_t *program, uint loaded_offset);
void pio_sm_put_blocking(pio_hw_t *pio, uint sm, uint32_t data);
uint32_t pio_sm_get_blocking(pio_hw_t *pio, uint sm);
void pio_sm_exec(pio_hw_t *pio, uint sm, uint instr);
void pio_sm_init(pio_hw_t *pio, uint sm, uint offset, const pio_sm_config *config);
void pio_sm_set_enabled(pio_hw_t *pio, uint sm, bool enabled);
void pio_sm_set_consecutive_pindirs(pio_hw_t *pio, uint sm, uint pin, uint count, bool is_out);
void pio_sm_set_clkdiv_int_frac(pio_hw_t *pio, uint sm, uint16_t div_int, uint8_t div_frac);
void sm_config_set_sideset_pins(pio_sm_config *config, uint sideset_base);
void sm_config_set_out_pins(pio_sm_config *config, uint out_base, uint out_count);
void sm_config_set_set_pins(pio_sm_config *config, uint set_base, uint set_count);
void sm_config_set_in_pins(pio_sm_config *config, uint in_base);
void sm_config_set_out_shift(pio_sm_config *config, bool shift_right, bool autopull,
                             uint pull_threshold);
void sm_config_set_in_shift(pio_sm_config *config, bool shift_right, bool autopush,
                            uint push_threshold);
/* ---- ph_fw_target_io.c: LED tick uses FreeRTOS + tinyUSB ---- */
typedef uint32_t TickType_t;
#define pdMS_TO_TICKS(ms) ((TickType_t)(ms))
TickType_t xTaskGetTickCount(void);
bool tud_ready(void);
uint32_t tud_vendor_read(void *buffer, uint32_t count);
uint32_t tud_cdc_n_available(uint8_t port);
uint32_t tud_cdc_n_read(uint8_t port, void *buffer, uint32_t count);
uint32_t tud_cdc_n_write_available(uint8_t port);
uint32_t tud_cdc_n_write(uint8_t port, const void *buffer, uint32_t count);
uint32_t tud_cdc_n_write_flush(uint8_t port);
bool tud_cdc_n_connected(uint8_t port);
static inline void __dmb(void)
{
}
/* pico/stdlib provides these through the SDK; the host needs them too. */
#ifndef MIN
#define MIN(a, b) ((a) < (b) ? (a) : (b))
#endif
#ifndef MAX
#define MAX(a, b) ((a) > (b) ? (a) : (b))
#endif
/* No-op CU debug pins (defined by hardware/gpio.h in the real SDK). */
#define CU_REGISTER_DEBUG_PINS(name)
#define CU_SELECT_DEBUG_PINS(name)
#define DEBUG_PINS_SET(name, pin)
#define DEBUG_PINS_CLR(name, pin)
#endif

# Bus self-test without wires (TTGO T-Display or any ESP32): the UART's TX and
# RX, and SPI's MOSI and MISO, are put on the same GPIO, so every byte sent comes
# back. Change the pins if 25-27 are wired to something on your board.
# UART1 loopback: TX and RX on the same pin
uart_init(25, 25, 115200)
print('uart_write:', uart_write(b'hello\x00\xff'), uart_write('AT\r\n'))
sleep_ms(5)
print('uart_any:', uart_any())
print('uart_read:', uart_read(64, 50))
print('empty read:', uart_read(8, 20))
uart_init(25, 25, 9600)
uart_write(bytes(range(40)))
data = uart_read(40, 200)
print('9600 baud, 40 bytes back:', len(data), data == bytes(range(40)))
# SPI3 loopback: MOSI and MISO on the same pin
spi_init(26, 27, 27, 2000, 0)
print('spi_transfer:', spi_transfer(b'\x12\x34\xab\xcd'))
spi_write(b'\x01\x02')
print('spi_read (fill 0xA5):', spi_read(3, 0xA5))
big = bytes(range(256)) * 2
print('512-byte transfer ok:', spi_transfer(big) == big)
# I2C with nothing connected
print('i2c_scan:', i2c_scan(), list(i2c_scan()))
try:
    i2c_read(0x3c, 2)
except OSError as e:
    print('i2c_read without a device: OSError')
try:
    uart_init(6, 7)
except ValueError:
    print('flash pins refused')

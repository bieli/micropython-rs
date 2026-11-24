# Scan the I2C bus (SDA = GPIO21, SCL = GPIO22) and print every address found
found = i2c_scan()                  # the addresses, as bytes
for addr in found:
    print('device at 0x%02x' % addr)
print('devices found:', len(found))

# Reading registers, e.g. an MPU6050 accelerometer at 0x68:
# if 0x68 in found:
#     print('WHO_AM_I =', hex(i2c_write_read(0x68, b'\x75', 1)[0]))
#     i2c_write(0x68, b'\x6b\x00')                 # wake it up
#     raw = i2c_write_read(0x68, b'\x3b', 6)       # X, Y, Z: 16-bit big-endian
#     x = raw[0] << 8 | raw[1]

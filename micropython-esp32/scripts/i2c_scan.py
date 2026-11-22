# Scan the I2C bus (SDA = GPIO21, SCL = GPIO22) and print every address found
found = 0
addr = 0x08
while addr < 0x78:
    if i2c_probe(addr):
        print('device at', addr)
        found += 1
    addr += 1
print('devices found:', found)

# Reading a register, e.g. the WHO_AM_I register of an MPU6050 at 0x68:
# if i2c_probe(0x68):
#     print('WHO_AM_I =', i2c_read_reg(0x68, 0x75))

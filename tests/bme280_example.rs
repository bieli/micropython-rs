//! The BME280 example (examples/bme280.rs): a native sensor module returning
//! floats, tuples, lists and dicts, its driver checked against the
//! datasheet's worked example and against the datasheet's floating-point
//! formulas.
#![cfg(all(feature = "float", feature = "format", feature = "stdlib"))]

#[path = "../examples/bme280.rs"]
#[allow(dead_code)]
mod example;

use example::{Bme280, SimulatedBme280, SCRIPT};

#[test]
fn the_example_script() {
    let (out, error) = example::run(SCRIPT, SimulatedBme280::new(0x76));
    assert_eq!(error, "");
    assert_eq!(
        out,
        "BME280 at 0x76 found at [118]\n\
         25.08 C  1006.53 hPa  43.9 %RH\n\
         dew point 11.9 C\n\
         ['humidity', 'pressure', 'temperature'] True\n\
         raw (519888, 415148, 28000) T1 27504\n\
         no sensor at 0x77\n"
    );
}

#[test]
fn values_and_types_in_python() {
    let (out, error) = example::run(
        "import bme280\n\
         r = bme280.read()\n\
         print(type(r).__name__, [type(v).__name__ for v in r], r[0])\n\
         i = bme280.info()\n\
         print(i['id'] == bme280.CHIP_ID, i['address'], len(i['calibration']), i['calibration']['H4'], i['calibration']['H5'])\n\
         print(type(bme280.scan()).__name__, type(bme280.read_dict()).__name__)\n",
        SimulatedBme280::new(0x76),
    );
    assert_eq!(error, "");
    assert_eq!(
        out,
        "tuple ['float', 'float', 'float'] 25.08\nTrue 118 18 313 50\nlist dict\n"
    );
}

#[test]
fn no_sensor_is_an_oserror() {
    // Nothing at 0x76 (the chip is at 0x77): read() fails, scan() finds it
    let (out, error) = example::run(
        "import bme280\nprint(bme280.scan())\ntry:\n    bme280.read()\nexcept OSError:\n    print('OSError')\n\
         bme280.init(0x77)\nprint(bme280.read()[0])\nbme280.init(300)\n",
        SimulatedBme280::new(0x77),
    );
    assert_eq!(out, "[119]\nOSError\n25.08\n");
    assert!(error.starts_with("ValueError"), "{}", error);
}

/// The datasheet's floating-point formulas (section 8.1), an independent
/// check of the integer ones the driver uses: (°C, Pa, %RH).
fn float_formulas(s: &Bme280, adc_t: f64, adc_p: f64, adc_h: f64) -> (f64, f64, f64) {
    let c = &s.calibration;
    let (t1, t2, t3) = (c.t1 as f64, c.t2 as f64, c.t3 as f64);
    let var1 = (adc_t / 16384.0 - t1 / 1024.0) * t2;
    let var2 = (adc_t / 131072.0 - t1 / 8192.0).powi(2) * t3;
    let t_fine = var1 + var2;
    let p = c.p.map(|v| v as f64);
    let mut v1 = t_fine / 2.0 - 64000.0;
    let mut v2 = v1 * v1 * p[5] / 32768.0;
    v2 += v1 * p[4] * 2.0;
    v2 = v2 / 4.0 + p[3] * 65536.0;
    v1 = (p[2] * v1 * v1 / 524288.0 + p[1] * v1) / 524288.0;
    v1 = (1.0 + v1 / 32768.0) * p[0];
    let mut pa = 1048576.0 - adc_p;
    pa = (pa - v2 / 4096.0) * 6250.0 / v1;
    let v1 = p[8] * pa * pa / 2147483648.0;
    let v2 = pa * p[7] / 32768.0;
    pa += (v1 + v2 + p[6]) / 16.0;
    let (h1, h2, h3) = (c.h1 as f64, c.h2 as f64, c.h3 as f64);
    let (h4, h5, h6) = (c.h4 as f64, c.h5 as f64, c.h6 as f64);
    let mut h = t_fine - 76800.0;
    h = (adc_h - (h4 * 64.0 + h5 / 16384.0 * h))
        * (h2 / 65536.0 * (1.0 + h6 / 67108864.0 * h * (1.0 + h3 / 67108864.0 * h)));
    h *= 1.0 - h1 * h / 524288.0;
    (t_fine / 5120.0, pa, h.clamp(0.0, 100.0))
}

#[test]
fn integer_formulas_match_the_datasheet() {
    let mut chip = SimulatedBme280::new(0x76);
    let sensor = Bme280::new(&mut chip, 0x76).unwrap();
    // The worked example: 25.08 °C, 100653.27 Pa (the float formula)
    let r = sensor.read(&mut chip).unwrap();
    assert_eq!(r.temperature, 2508);
    assert!(
        (r.pressure as f64 / 256.0 - 100_653.27).abs() < 0.1,
        "{}",
        r.pressure
    );
    // A range of readings: about -30 to +60 °C, 300-1100 hPa, 0-100 %RH
    for adc_t in (380_000..620_000).step_by(20_000) {
        for adc_p in (250_000..560_000).step_by(30_000) {
            for adc_h in (15_000..40_000).step_by(5_000) {
                chip.set_adc(adc_t, adc_p, adc_h);
                let r = sensor.read(&mut chip).unwrap();
                let (t, p, h) = float_formulas(&sensor, adc_t as f64, adc_p as f64, adc_h as f64);
                let what = (adc_t, adc_p, adc_h);
                assert!(
                    (r.temperature as f64 / 100.0 - t).abs() < 0.01,
                    "{:?}",
                    what
                );
                assert!(
                    (r.pressure as f64 / 256.0 - p).abs() < 1.0,
                    "{:?} {} {}",
                    what,
                    r.pressure,
                    p
                );
                assert!((r.humidity as f64 / 1024.0 - h).abs() < 0.05, "{:?}", what);
            }
        }
    }
}

#[test]
fn calibration_with_negative_humidity_values() {
    // H4 and H5 are 12-bit signed values sharing the register 0xE5
    let mut chip = SimulatedBme280::new(0x76);
    let (h4, h5) = (-300i16, -45i16);
    chip.regs[0xE4] = (h4 >> 4) as u8;
    chip.regs[0xE5] = ((h5 as u8 & 0x0F) << 4) | (h4 as u8 & 0x0F);
    chip.regs[0xE6] = (h5 >> 4) as u8;
    let sensor = Bme280::new(&mut chip, 0x76).unwrap();
    assert_eq!((sensor.calibration.h4, sensor.calibration.h5), (-300, -45));
    assert_eq!(sensor.calibration.t3, -1000);
    assert_eq!(sensor.calibration.p[8], 6000);
}

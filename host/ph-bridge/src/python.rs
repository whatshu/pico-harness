//! PyO3 bindings — the PH Bridge as a Python extension module.
//!
//! Built behind `feature = "python"` and packaged with maturin; the native
//! module is loaded as `phbridge._native` (optional; the pure-Python
//! `host/phbridge` package is the always-available interface).

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::client::SerialTransport;
use crate::{PhClient, PhError};

fn pyerr(e: PhError) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}

/// Open the CDC `PH Terminal` serial and return a device handle.
#[pyfunction]
fn open_serial(port: &str) -> PyResult<PyDevice> {
    Ok(PyDevice {
        inner: PhClient::open_serial(port).map_err(pyerr)?,
    })
}

/// List the probe's control ttyACM ports found on this host.
#[pyfunction]
fn scan() -> PyResult<Vec<String>> {
    Ok(crate::device::scan()
        .into_iter()
        .filter_map(|d| d.terminal_tty.map(|p| p.to_string_lossy().into_owned()))
        .collect())
}

#[pyclass(unsendable)]
struct PyDevice {
    inner: PhClient<SerialTransport>,
}

#[pymethods]
impl PyDevice {
    fn power_set(&mut self, on: bool) -> PyResult<()> {
        self.inner.power_set(on).map_err(pyerr)
    }

    fn measure(&mut self) -> PyResult<(i32, i32, i32, i32)> {
        let m = self.inner.measure().map_err(pyerr)?;
        Ok((m.shunt_uv, m.vbus_uv, m.current_ua, m.power_uw))
    }

    fn get_info(&mut self) -> PyResult<String> {
        Ok(self.inner.get_info().map_err(pyerr)?.info)
    }

    fn get_status(&mut self) -> PyResult<(bool, bool, bool, bool, u32)> {
        let s = self.inner.get_status().map_err(pyerr)?;
        Ok((s.power_enabled, s.power_good, s.ina_alert, s.work_fault, s.vbus_mv))
    }

    fn console_tx(&mut self, data: &[u8]) -> PyResult<()> {
        self.inner.console_tx(data).map_err(pyerr)
    }

    fn term_config(&mut self, baud: u32) -> PyResult<u32> {
        self.inner.term_config(baud, 8, 1, 0).map_err(pyerr)
    }

    fn debug_bootsel(&mut self) -> PyResult<()> {
        self.inner.debug_bootsel().map_err(pyerr)
    }
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(open_serial, m)?)?;
    m.add_function(wrap_pyfunction!(scan, m)?)?;
    m.add_class::<PyDevice>()?;
    Ok(())
}
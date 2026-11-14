use std::{
    ffi::{CString, c_char, c_int, c_void},
    fmt::Write,
    io,
    ptr::{NonNull, null, null_mut},
    slice,
    sync::{Arc, OnceLock},
    time::Duration,
};

use libloading::Library;
use rustix::io::Errno;

const LIBSYSTEMD_SONAME: &str = "libsystemd.so.0";
const ALL_JOURNALS: c_int = 0;

#[repr(C)]
struct SystemdId128 {
    bytes: [u8; 16],
}

struct JournalFunctions {
    open: unsafe extern "C" fn(*mut *mut c_void, c_int) -> c_int,
    close: unsafe extern "C" fn(*mut c_void),
    add_match: unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> c_int,
    add_disjunction: unsafe extern "C" fn(*mut c_void) -> c_int,
    add_conjunction: unsafe extern "C" fn(*mut c_void) -> c_int,
    get_fd: unsafe extern "C" fn(*mut c_void) -> c_int,
    seek_tail: unsafe extern "C" fn(*mut c_void) -> c_int,
    previous: unsafe extern "C" fn(*mut c_void) -> c_int,
    get_data:
        unsafe extern "C" fn(*mut c_void, *const c_char, *mut *const c_void, *mut usize) -> c_int,
    wait: unsafe extern "C" fn(*mut c_void, u64) -> c_int,
    get_boot_id: unsafe extern "C" fn(*mut SystemdId128) -> c_int,
    _library: Library,
}

static LOADED_FUNCTIONS: OnceLock<Result<JournalFunctions, Arc<libloading::Error>>> =
    OnceLock::new();

fn load_journal_functions() -> Result<JournalFunctions, libloading::Error> {
    unsafe {
        let library = Library::new(LIBSYSTEMD_SONAME)?;
        Ok(JournalFunctions {
            open: *library.get("sd_journal_open")?,
            close: *library.get("sd_journal_close")?,
            add_match: *library.get("sd_journal_add_match")?,
            add_disjunction: *library.get("sd_journal_add_disjunction")?,
            add_conjunction: *library.get("sd_journal_add_conjunction")?,
            get_fd: *library.get("sd_journal_get_fd")?,
            seek_tail: *library.get("sd_journal_seek_tail")?,
            previous: *library.get("sd_journal_previous")?,
            get_data: *library.get("sd_journal_get_data")?,
            wait: *library.get("sd_journal_wait")?,
            get_boot_id: *library.get("sd_id128_get_boot")?,
            _library: library,
        })
    }
}

fn journal_functions() -> io::Result<&'static JournalFunctions> {
    LOADED_FUNCTIONS
        .get_or_init(|| load_journal_functions().map_err(Arc::new))
        .as_ref()
        .map_err(|load_error| io::Error::new(io::ErrorKind::NotFound, Arc::clone(load_error)))
}

fn checked_return_code(code: c_int) -> io::Result<c_int> {
    if code < 0 {
        Err(io::Error::from_raw_os_error(-code))
    } else {
        Ok(code)
    }
}

pub(crate) enum JournalStep {
    Moved,
    AtOldestEntry,
}

pub(crate) struct OpenJournal {
    handle: NonNull<c_void>,
    functions: &'static JournalFunctions,
}

unsafe impl Send for OpenJournal {}

impl OpenJournal {
    pub(crate) fn open_all() -> io::Result<Self> {
        let functions = journal_functions()?;
        let mut handle = null_mut();
        checked_return_code(unsafe { (functions.open)(&raw mut handle, ALL_JOURNALS) })?;
        let handle = NonNull::new(handle).ok_or_else(|| {
            io::Error::other("sd_journal_open succeeded without returning a journal")
        })?;
        Ok(Self { handle, functions })
    }

    pub(crate) fn add_match(&mut self, field: &str, value: &str) -> io::Result<()> {
        let field_match = format!("{field}={value}");
        checked_return_code(unsafe {
            (self.functions.add_match)(
                self.handle.as_ptr(),
                field_match.as_ptr().cast(),
                field_match.len(),
            )
        })?;
        Ok(())
    }

    pub(crate) fn add_disjunction(&mut self) -> io::Result<()> {
        checked_return_code(unsafe { (self.functions.add_disjunction)(self.handle.as_ptr()) })?;
        Ok(())
    }

    pub(crate) fn add_conjunction(&mut self) -> io::Result<()> {
        checked_return_code(unsafe { (self.functions.add_conjunction)(self.handle.as_ptr()) })?;
        Ok(())
    }

    pub(crate) fn watch_for_new_entries(&mut self) -> io::Result<()> {
        checked_return_code(unsafe { (self.functions.get_fd)(self.handle.as_ptr()) })?;
        Ok(())
    }

    pub(crate) fn seek_tail(&mut self) -> io::Result<()> {
        checked_return_code(unsafe { (self.functions.seek_tail)(self.handle.as_ptr()) })?;
        Ok(())
    }

    pub(crate) fn step_back(&mut self) -> io::Result<JournalStep> {
        let moved =
            checked_return_code(unsafe { (self.functions.previous)(self.handle.as_ptr()) })?;
        Ok(match moved {
            0 => JournalStep::AtOldestEntry,
            _ => JournalStep::Moved,
        })
    }

    pub(crate) fn field_bytes(&mut self, field: &str) -> io::Result<Option<Vec<u8>>> {
        let field_name = CString::new(field).map_err(io::Error::other)?;
        let mut data = null();
        let mut length = 0;
        let code = unsafe {
            (self.functions.get_data)(
                self.handle.as_ptr(),
                field_name.as_ptr(),
                &raw mut data,
                &raw mut length,
            )
        };
        if code == -Errno::NOENT.raw_os_error() {
            return Ok(None);
        }
        checked_return_code(code)?;
        let entry_field = unsafe { slice::from_raw_parts(data.cast::<u8>(), length) };
        let prefix_length = field.len() + 1;
        Ok(Some(
            entry_field
                .get(prefix_length..)
                .unwrap_or_default()
                .to_vec(),
        ))
    }

    pub(crate) fn wait(&mut self, patience: Duration) -> io::Result<()> {
        let micros = u64::try_from(patience.as_micros()).unwrap_or(u64::MAX);
        checked_return_code(unsafe { (self.functions.wait)(self.handle.as_ptr(), micros) })?;
        Ok(())
    }
}

impl Drop for OpenJournal {
    fn drop(&mut self) {
        unsafe { (self.functions.close)(self.handle.as_ptr()) };
    }
}

pub(crate) fn current_boot_id() -> io::Result<String> {
    let functions = journal_functions()?;
    let mut boot_id = SystemdId128 { bytes: [0; 16] };
    checked_return_code(unsafe { (functions.get_boot_id)(&raw mut boot_id) })?;
    Ok(boot_id
        .bytes
        .iter()
        .fold(String::with_capacity(32), |mut hex_digits, byte| {
            write!(hex_digits, "{byte:02x}").expect("writing to a String does not fail");
            hex_digits
        }))
}

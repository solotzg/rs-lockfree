//! Definition of error and status.

use std::fmt;

/// Status of `HazardEpoch`
#[derive(PartialEq, Copy, Clone, Debug)]
pub enum Status {
    /// Success
    Success,
    /// Current thread has already assigned a version handle
    Busy,
    /// Thread number overflow
    ThreadNumOverflow,
    /// Invalid parameter
    InvalidParam,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

mod test {

    #[test]
    fn test_base() {
        use error::Status;

        let s = Status::Success;
        let a = format!("{}", s);
        assert_eq!(a, "Success");
    }
}

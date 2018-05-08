use std::fmt;

#[derive(PartialEq, Copy, Clone, Debug)]
pub enum Status {
    Success,
    Busy,
    TooManyThreads,
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

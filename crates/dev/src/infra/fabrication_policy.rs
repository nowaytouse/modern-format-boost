use std::env;
use std::process;

pub fn fail_closed_training_enabled() -> bool {
    let value = env::var("MFB_TRAINING_FAIL_CLOSED")
        .unwrap_or_else(|_| "1".to_string())
        .trim()
        .to_lowercase();
    !matches!(value.as_str(), "0" | "false" | "no" | "off")
}

pub fn training_quality_exit(code: i32, message: &str) -> ! {
    eprintln!("{}", message);
    process::exit(code);
}

pub fn re_raise_training_exception(context: &str, exc_msg: &str) -> ! {
    panic!("{}: {}", context, exc_msg);
}

pub fn run_training_except_policy<F>(context: &str, exc_msg: &str, mut on_retry: Option<F>) -> !
where
    F: FnMut(),
{
    if fail_closed_training_enabled() {
        re_raise_training_exception(context, exc_msg);
    }
    if let Some(ref mut retry_fn) = on_retry {
        retry_fn();
        process::exit(1); // We can't actually resume after panic without catching, so we exit if not handled.
    }
    re_raise_training_exception(context, exc_msg);
}

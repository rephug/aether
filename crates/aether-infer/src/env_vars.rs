//! Environment variable reads for provider and key loading.
//!
//! Every read goes through [`var`]. Unit tests set values with
//! [`overrides::set`] instead of `std::env::set_var`: the override lives in a
//! thread-local map for the duration of the returned guard, so tests running in
//! parallel never mutate the process environment, which is undefined behavior
//! while other threads may read it. Overrides are visible only on the thread that
//! set them, so code under test must read keys on the test's own thread.

/// The value of the environment variable `name`, if set and valid Unicode.
pub(crate) fn var(name: &str) -> Option<String> {
    #[cfg(test)]
    if let Some(value) = overrides::get(name) {
        return value;
    }
    std::env::var(name).ok()
}

#[cfg(test)]
pub(crate) mod overrides {
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static VALUES: RefCell<HashMap<String, Option<String>>> = RefCell::new(HashMap::new());
    }

    /// Restores the previous override (or its absence) when dropped.
    #[must_use = "the override lasts only while the guard is alive"]
    pub(crate) struct ScopedVar {
        name: String,
        previous: Option<Option<String>>,
    }

    impl Drop for ScopedVar {
        fn drop(&mut self) {
            VALUES.with(|values| {
                let mut values = values.borrow_mut();
                match self.previous.take() {
                    Some(previous) => values.insert(self.name.clone(), previous),
                    None => values.remove(&self.name),
                };
            });
        }
    }

    /// Makes `name` read as `value` on this thread until the guard is dropped.
    pub(crate) fn set(name: &str, value: &str) -> ScopedVar {
        replace(name, Some(value.to_owned()))
    }

    /// Makes `name` read as unset on this thread until the guard is dropped.
    pub(crate) fn unset(name: &str) -> ScopedVar {
        replace(name, None)
    }

    pub(super) fn get(name: &str) -> Option<Option<String>> {
        VALUES.with(|values| values.borrow().get(name).cloned())
    }

    fn replace(name: &str, value: Option<String>) -> ScopedVar {
        let previous = VALUES.with(|values| values.borrow_mut().insert(name.to_owned(), value));
        ScopedVar {
            name: name.to_owned(),
            previous,
        }
    }

    mod tests {
        use super::super::var;
        use super::{set, unset};

        #[test]
        fn overrides_are_scoped_to_the_guard_and_thread() {
            let name = "AETHER_TEST_ENV_VARS_SCOPED_ZZZZZ";
            assert_eq!(var(name), None);
            {
                let _value = set(name, "inner");
                assert_eq!(var(name).as_deref(), Some("inner"));
                // The process environment is never touched.
                assert!(std::env::var(name).is_err());
                let other_thread = std::thread::spawn(move || var(name)).join().expect("join");
                assert_eq!(other_thread, None);
                {
                    let _unset = unset(name);
                    assert_eq!(var(name), None);
                }
                assert_eq!(var(name).as_deref(), Some("inner"));
            }
            assert_eq!(var(name), None);
        }
    }
}

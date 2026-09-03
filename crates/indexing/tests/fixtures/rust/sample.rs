//! Free function at module scope.
fn free_function() {
    let x = 1;
    let _ = x;
}

/// Returns a greeting for the given name.
fn documented(name: &str) -> String {
    format!("hello {name}")
}

struct Tracker {
    value: i32,
}

impl Tracker {
    fn new() -> Self {
        Self { value: 0 }
    }

    fn update(&mut self) {
        self.value += 1;
    }
}

mod nested {
    pub fn inner() {}
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert!(true);
    }
}

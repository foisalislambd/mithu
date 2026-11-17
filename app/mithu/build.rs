fn main() {
    // Servo and SpiderMonkey need more than the 1MB Windows main-thread stack.
    // The event loop has to stay on that thread, so the reserve is raised here
    // instead of moving the loop onto a worker (which winit rejects).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg=/STACK:33554432");
    }
}

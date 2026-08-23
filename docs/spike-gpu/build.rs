fn main() {
    // Export the magic statics from the EXE's export table; the NVIDIA and AMD
    // drivers scan for these to route a hybrid-graphics process to the dGPU.
    println!("cargo:rustc-link-arg=/EXPORT:NvOptimusEnablement,DATA");
    println!("cargo:rustc-link-arg=/EXPORT:AmdPowerXpressRequestHighPerformance,DATA");
}

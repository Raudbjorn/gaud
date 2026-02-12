mod fake;
mod notrack;
mod registry;

pub use registry::{
    MemoryReporter, cleanup_memory_reporters, memory_reporters_allocated_by_name,
    memory_reporters_allocated_total, register_memory_reporter,
};

// --------------------------------------------------
// No global allocator
// --------------------------------------------------

#[cfg(not(feature = "allocator"))]
pub static ALLOC: fake::FakeAlloc = fake::FakeAlloc::new();

// --------------------------------------------------
// Global allocator
// --------------------------------------------------

#[cfg(all(
    feature = "allocator",
    not(any(unix, windows)),
    not(all(
        any(target_arch = "x86_64", target_arch = "x86"),
        any(target_os = "linux", target_os = "macos"),
        not(target_env = "msvc"),
    ))
))]
#[global_allocator]
pub static ALLOC: notrack::NotrackAlloc<std::alloc::System> =
    notrack::NotrackAlloc::new(std::alloc::System);

#[cfg(all(
    feature = "allocator",
    any(unix, windows),
    not(all(
        any(target_arch = "x86_64", target_arch = "x86"),
        any(target_os = "linux", target_os = "macos"),
        not(target_env = "msvc"),
    ))
))]
#[global_allocator]
pub static ALLOC: notrack::NotrackAlloc<mimalloc::MiMalloc> =
    notrack::NotrackAlloc::new(mimalloc::MiMalloc);

#[cfg(all(
    feature = "allocator",
    all(
        any(target_arch = "x86_64", target_arch = "x86"),
        any(target_os = "linux", target_os = "macos"),
        not(target_env = "msvc"),
    )
))]
#[global_allocator]
pub static ALLOC: notrack::NotrackAlloc<jemallocator::Jemalloc> =
    notrack::NotrackAlloc::new(jemallocator::Jemalloc);

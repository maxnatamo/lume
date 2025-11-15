pub mod array;
pub mod io;
pub mod mem;
pub mod string;

/// Retrieves of the type metadata of the first type parameter.
///
/// Since Lume passes the metadata of type arguments after all other parameters,
/// we can safely return the first input pointer.
pub extern "C" fn type_of(metadata: *const ()) -> *const () {
    metadata
}

pub extern "C" fn backtrace() {
    println!("Stack trace:");

    backtrace::trace(|frame| {
        let mut name = None;
        let mut addr = None;

        backtrace::resolve(frame.ip(), |symbol| {
            name = symbol.name().map(|n| n.to_string());
            addr = symbol.addr();
        });

        println!(
            "  {} in {}",
            addr.map_or_else(|| format!("{:p}", frame.ip()), |addr| format!("{addr:p}")),
            name.unwrap_or_else(|| String::from("??")),
        );

        true
    });
}

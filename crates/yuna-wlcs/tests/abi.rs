//! The shim's C declarations (`src/abi.rs`) against WLCS's own headers: a C program compiled
//! against `$YUNA_WLCS_INCLUDE`, which the development shell sets, prints each struct's size and
//! alignment, each field's offset and size, and each version WLCS defines, which must be Rust's.
//! Both lists come from one table, [`layouts`], so that neither can leave a field out.
//!
//! WLCS's `pointer.h` and `touch.h` take `wl_fixed_t` from libwayland-client's
//! `wayland-client-core.h`, whose headers the shell does not hold: the test gives the program a
//! header of its own that declares it as libwayland does, `int32_t` (`wayland-util.h`).

use std::fmt::Write as _;
use std::mem::{align_of, offset_of, size_of, size_of_val};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr;

use yuna_wlcs::abi::{
    DISPLAY_SERVER_VERSION, INTEGRATION_DESCRIPTOR_VERSION, POINTER_VERSION,
    SERVER_INTEGRATION_VERSION, TOUCH_VERSION, WlcsDisplayServer, WlcsExtensionDescriptor,
    WlcsIntegrationDescriptor, WlcsPointer, WlcsServerIntegration, WlcsTouch,
};

/// What the C program prints, and Rust's own: one line each.
#[derive(Default)]
struct Layouts {
    /// The C program's `printf`s.
    c: String,
    /// The lines Rust expects: `<struct> <size> <alignment>`, `<struct>.<field> <offset>
    /// <size>`, `<macro> <value>`.
    rust: Vec<String>,
}

/// Adds `$ty` and each of its `$field`s, read on `$value`, a value of it, to both lists.
macro_rules! layout {
    ($layouts:ident, $ty:ident = $value:expr; $($field:ident),+ $(,)?) => {{
        let value: $ty = $value;
        let name = stringify!($ty);
        $layouts.rust.push(format!("{name} {} {}", size_of::<$ty>(), align_of::<$ty>()));
        let _ = writeln!(
            $layouts.c,
            "    printf(\"%s %zu %zu\\n\", \"{name}\", sizeof({name}), _Alignof({name}));"
        );
        $(
            let field = stringify!($field);
            $layouts.rust.push(format!(
                "{name}.{field} {} {}",
                offset_of!($ty, $field),
                size_of_val(&value.$field)
            ));
            let _ = writeln!(
                $layouts.c,
                "    printf(\"%s %zu %zu\\n\", \"{name}.{field}\", offsetof({name}, {field}), \
                 sizeof((({name} *)0)->{field}));"
            );
        )+
    }};
}

/// Adds the version WLCS's header names `$macro`, which Rust holds as `$value`, to both lists.
macro_rules! version {
    ($layouts:ident, $macro:ident, $value:expr) => {{
        let name = stringify!($macro);
        $layouts.rust.push(format!("{name} {}", $value));
        let _ = writeln!(
            $layouts.c,
            "    printf(\"%s %u\\n\", \"{name}\", (unsigned) {name});"
        );
    }};
}

/// Every struct of the ABI, field by field, and every version.
fn layouts() -> Layouts {
    let mut layouts = Layouts::default();
    layout!(layouts, WlcsServerIntegration = WlcsServerIntegration {
        version: 0,
        create_server: None,
        destroy_server: None,
    }; version, create_server, destroy_server);
    layout!(layouts, WlcsDisplayServer = WlcsDisplayServer {
        version: 0,
        start: None,
        stop: None,
        create_client_socket: None,
        position_window_absolute: None,
        create_pointer: None,
        create_touch: None,
        get_descriptor: None,
        start_on_this_thread: None,
    }; version, start, stop, create_client_socket, position_window_absolute, create_pointer,
        create_touch, get_descriptor, start_on_this_thread);
    layout!(layouts, WlcsIntegrationDescriptor = WlcsIntegrationDescriptor {
        version: 0,
        num_extensions: 0,
        supported_extensions: ptr::null(),
    }; version, num_extensions, supported_extensions);
    layout!(layouts, WlcsExtensionDescriptor = WlcsExtensionDescriptor {
        name: ptr::null(),
        version: 0,
    }; name, version);
    layout!(layouts, WlcsPointer = WlcsPointer {
        version: 0,
        move_absolute: None,
        move_relative: None,
        button_up: None,
        button_down: None,
        destroy: None,
    }; version, move_absolute, move_relative, button_up, button_down, destroy);
    layout!(layouts, WlcsTouch = WlcsTouch {
        version: 0,
        touch_down: None,
        touch_move: None,
        touch_up: None,
        destroy: None,
    }; version, touch_down, touch_move, touch_up, destroy);
    version!(
        layouts,
        WLCS_SERVER_INTEGRATION_VERSION,
        SERVER_INTEGRATION_VERSION
    );
    version!(layouts, WLCS_DISPLAY_SERVER_VERSION, DISPLAY_SERVER_VERSION);
    version!(
        layouts,
        WLCS_INTEGRATION_DESCRIPTOR_VERSION,
        INTEGRATION_DESCRIPTOR_VERSION
    );
    version!(layouts, WLCS_POINTER_VERSION, POINTER_VERSION);
    version!(layouts, WLCS_TOUCH_VERSION, TOUCH_VERSION);
    layouts
}

/// WLCS's headers' folder, from the development shell.
fn wlcs_include() -> PathBuf {
    match std::env::var_os("YUNA_WLCS_INCLUDE") {
        Some(folder) => PathBuf::from(folder),
        None => panic!(
            "YUNA_WLCS_INCLUDE is not set: run the test in the development shell (nix develop), \
             which points it at WLCS's headers"
        ),
    }
}

/// Compiles `source` with `cc` into `folder`, the headers of `include` and `stubs` on its path,
/// and returns what the program printed.
fn compile_and_run(folder: &Path, source: &str, include: &Path, stubs: &Path) -> String {
    let file = folder.join("abi.c");
    let program = folder.join("abi");
    std::fs::write(&file, source).expect("the C program is written");
    let compiled = Command::new("cc")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(&program)
        .arg("-I")
        .arg(stubs)
        .arg("-I")
        .arg(include)
        .arg(&file)
        .output()
        .unwrap_or_else(|error| {
            panic!("cc did not run ({error}): run the test in the development shell, which has it")
        });
    assert!(
        compiled.status.success(),
        "cc failed on WLCS's headers in {}:\n{}",
        include.display(),
        String::from_utf8_lossy(&compiled.stderr)
    );
    let run = Command::new(&program).output().expect("the C program runs");
    assert!(run.status.success(), "the C program failed: {}", run.status);
    String::from_utf8(run.stdout).expect("the C program prints text")
}

#[test]
fn the_declarations_are_wlcs_headers_layout() {
    let include = wlcs_include();
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR")).join("yuna-wlcs-abi");
    let stubs = folder.join("stubs");
    std::fs::create_dir_all(&stubs).expect("the test's folder is made");
    std::fs::write(
        stubs.join("wayland-client-core.h"),
        "/* libwayland-client's fixed-point number, as wayland-util.h declares it. */\n\
         #include <stdint.h>\n\
         typedef int32_t wl_fixed_t;\n",
    )
    .expect("the stub header is written");
    let layouts = layouts();
    let source = format!(
        "#include <stddef.h>\n\
         #include <stdio.h>\n\
         #include <wlcs/display_server.h>\n\
         #include <wlcs/pointer.h>\n\
         #include <wlcs/touch.h>\n\
         \n\
         int main(void)\n\
         {{\n\
         {}    return 0;\n\
         }}\n",
        layouts.c
    );
    let printed = compile_and_run(&folder, &source, &include, &stubs);
    let c: Vec<&str> = printed.lines().collect();
    let rust: Vec<&str> = layouts.rust.iter().map(String::as_str).collect();
    let differing: Vec<String> = c
        .iter()
        .zip(&rust)
        .filter(|(c, rust)| c != rust)
        .map(|(c, rust)| format!("C: {c}; Rust: {rust}"))
        .collect();
    assert_eq!(c.len(), rust.len(), "the C program printed {printed}");
    assert!(
        differing.is_empty(),
        "src/abi.rs differs from WLCS's headers in {}:\n{}",
        include.display(),
        differing.join("\n")
    );
}

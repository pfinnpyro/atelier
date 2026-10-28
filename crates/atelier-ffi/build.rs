//! Writes the C# P/Invoke declarations for `src/lib.rs` into the .NET project,
//! so the two sides cannot drift apart. CI fails if the committed file is stale.

fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");
    csbindgen::Builder::default()
        .input_extern_file("src/lib.rs")
        .csharp_dll_name("atelier")
        .csharp_namespace("Atelier.Interop")
        .csharp_class_name("NativeMethods")
        .csharp_class_accessibility("internal")
        .csharp_use_function_pointer(false)
        .generate_csharp_file("../../dotnet/src/Atelier.Net/Interop/NativeMethods.g.cs")
        .expect("cannot write the C# bindings");
}

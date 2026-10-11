// Applies the native async error-provenance extension to exact pinned sources.
// All edits are isolated; dependency caches are never modified.
package main

import (
	"crypto/sha256"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

func main() {
	if len(os.Args) != 3 {
		panic("usage: patch-vm-boundary WASMVM COSMWASM")
	}
	vm, core := os.Args[1], os.Args[2]
	patch := func(root, path, old, new string) {
		p := filepath.Join(root, path)
		b, e := os.ReadFile(p)
		if e != nil {
			panic(e)
		}
		if strings.Count(string(b), old) != 1 {
			panic(fmt.Sprintf("anchor drift %s: %s", p, old))
		}
		if e = os.WriteFile(p, []byte(strings.Replace(string(b), old, new, 1)), 0644); e != nil {
			panic(e)
		}
	}
	patch(vm, "libwasmvm/Cargo.toml", `git = "https://github.com/CosmWasm/cosmwasm.git", rev = "v2.2.2", features = [
  "staking",
  "stargate",
  "iterator",
] }
cosmwasm-vm`, fmt.Sprintf(`path = %q, features = ["staking", "stargate", "iterator"] }
cosmwasm-vm`, filepath.Join(core, "packages/std")))
	patch(vm, "libwasmvm/Cargo.toml", `git = "https://github.com/CosmWasm/cosmwasm.git", rev = "v2.2.2"`, fmt.Sprintf("path = %q", filepath.Join(core, "packages/vm")))
	// Preserve every transitive version/checksum. Only the six pinned workspace
	// packages change from git to local paths; Cargo must never resolve updates.
	lockPath := filepath.Join(vm, "libwasmvm/Cargo.lock")
	lock, err := os.ReadFile(lockPath)
	if err != nil {
		panic(err)
	}
	if fmt.Sprintf("%x", sha256.Sum256(lock)) != "d185ec9e660b9a607cecbaa69e3adc17d05ae7d60a24bccea75ca2e2210198af" {
		panic("pinned native Cargo.lock changed")
	}
	const gitSource = "source = \"git+https://github.com/CosmWasm/cosmwasm.git?rev=v2.2.2#dd90b6f9cc9012fe78d436b841f2f2d93b65ccd3\"\n"
	if strings.Count(string(lock), gitSource) != 6 {
		panic("expected six pinned workspace sources")
	}
	locked := strings.ReplaceAll(string(lock), gitSource, "")
	locked = strings.Replace(locked, "version = 3\n", "version = 4\n", 1)
	if fmt.Sprintf("%x", sha256.Sum256([]byte(locked))) != "f0562bb3bc97c169f03c8c7c719c5291eb49147432148b0c17caeb73af0c6624" {
		panic("unexpected native lockfile transformation")
	}
	if err := os.WriteFile(lockPath, []byte(locked), 0644); err != nil {
		panic(err)
	}
	fmt.Printf("Native Cargo.lock SHA256: %x\n", sha256.Sum256([]byte(locked)))
	path := "packages/vm/src/errors/vm_error.rs"
	b, e := os.ReadFile(filepath.Join(core, path))
	if e != nil {
		panic(e)
	}
	start := strings.Index(string(b), "        // Do not use the Display implementation")
	end := strings.Index(string(b)[start:], "\n    }\n}") + start
	patch(core, path, string(b)[start:end], `        // Preserve typed host failures across Wasmer instead of formatting them.
        let original = match original.downcast::<VmError>() {
            Ok(error) => return error,
            Err(error) => error,
        };
        match original.to_trap() {
            Some(code) => VmError::runtime_err(format!("Wasm trap: {}", code.message())),
            None => VmError::generic_err("unclassified Wasmer runtime failure"),
        }`)
	patch(core, path, `        let msg: String = original.to_string();
        wasmer::RuntimeError::new(msg)`, `        wasmer::RuntimeError::user(Box::new(original))`)
	patch(core, "packages/vm/src/calls.rs", `VmError::generic_err(format!("Query response must be valid JSON. {e}"))`, `VmError::parse_err("QueryResponse", e)`)
	patch(core, "packages/vm/src/imports.rs", `VmError::generic_err(format!(
                "{kind} too big. Tried to write {length} bytes to storage, limit is {max_length}."
            ))`, `VmError::aborted(format!(
                "{kind} too big. Tried to write {length} bytes to storage, limit is {max_length}."
            ))`)
	patch(core, path, "    // constructors", `    #[test]
    fn native_async_preserves_backend_provenance() {
        let error = VmError::backend_err(BackendError::foreign_panic());
        let roundtrip = VmError::from(wasmer::RuntimeError::from(error));
        assert!(matches!(roundtrip, VmError::BackendErr { source: BackendError::ForeignPanic {}, .. }));
        let unknown = VmError::from(wasmer::RuntimeError::new("guest-looking text"));
        assert!(matches!(unknown, VmError::GenericErr { .. }));
    }

    // constructors`)
	patch(core, path, "impl From<wasmer::CompileError> for VmError {", `impl From<wasmer::InstantiationError> for VmError {
    fn from(original: wasmer::InstantiationError) -> Self {
        use wasmer::{InstantiationError as I, LinkError as L};
        use wasmer_types::ImportError as E;
        match original {
            I::Start(error) | I::Link(L::Trap(error)) => error.into(),
            I::Link(L::Import(_, _, E::IncompatibleType(..) | E::UnknownImport(..))) =>
                VmError::static_validation_err("guest import type mismatch or missing import"),
            other => VmError::instantiation_err(other.to_string()),
        }
    }
}

impl From<wasmer::CompileError> for VmError {`)
	patch(core, "packages/vm/src/instance.rs", `.map_err(|original| {
                VmError::instantiation_err(format!("Error instantiating module: {original}"))
            })?`, `.map_err(VmError::from)?`)
	patch(core, "packages/vm/src/parsed_wasm.rs", "            match p {", `            match p {
                Payload::StartSection { .. } => return Err(VmError::static_validation_err("Wasm start sections are not supported by native async ABI1")),`)
	patch(core, "packages/vm/src/wasm_backend/compile.rs", "    let module = Module::new(&engine, code)?;", `    // Also enforce on raw-Wasm cache rebuild and unchecked code import.
    for payload in wasmer::wasmparser::Parser::new(0).parse_all(code) {
        if matches!(payload?, wasmer::wasmparser::Payload::StartSection { .. }) {
            return Err(crate::VmError::static_validation_err("Wasm start sections are not supported by native async ABI1"));
        }
    }
    let module = Module::new(&engine, code)?;`)
	patch(core, "packages/vm/src/modules/file_system_cache.rs", `const MODULE_SERIALIZATION_VERSION: &str = "v20";`, `const MODULE_SERIALIZATION_VERSION: &str = "v20-native-async-error-abi1";`)
	patch(core, path, "    // constructors", `    #[test]
    fn native_async_instantiation_categories() {
        use wasmer::{InstantiationError as I, LinkError as L};
        use wasmer_types::{ImportError as E, ExternType, FunctionType};
        let host = wasmer::RuntimeError::from(VmError::backend_err(BackendError::foreign_panic()));
        assert!(matches!(VmError::from(I::Start(host)), VmError::BackendErr { .. }));
        let ty = ExternType::Function(FunctionType::new([], []));
        assert!(matches!(VmError::from(I::Link(L::Import("env".into(), "db_read".into(), E::UnknownImport(ty)))), VmError::StaticValidationErr { .. }));
        assert!(matches!(VmError::from(I::CpuFeature("missing".into())), VmError::InstantiationErr { .. }));
        assert!(matches!(VmError::from(I::DifferentStores), VmError::InstantiationErr { .. }));
        assert!(matches!(VmError::from(I::Link(L::Resource("allocation".into()))), VmError::InstantiationErr { .. }));
        let wasm = wat::parse_str("(module (func $s unreachable) (start $s))").unwrap();
        assert!(matches!(crate::parsed_wasm::ParsedWasm::parse(&wasm), Err(VmError::StaticValidationErr { .. })));
        let engine = crate::wasm_backend::make_compiling_engine(None);
        assert!(matches!(crate::wasm_backend::compile(&engine, &wasm), Err(VmError::StaticValidationErr { .. })));
        // Bypass admission only in this unit test to exercise native Start trap classification.
        let module = wasmer::Module::new(&engine, &wasm).unwrap();
        let mut store = wasmer::Store::new(engine);
        let error = wasmer::Instance::new(&mut store, &module, &wasmer::imports! {}).unwrap_err();
        assert!(matches!(VmError::from(error), VmError::RuntimeErr { .. }));
    }

    // constructors`)
	patch(vm, "libwasmvm/src/error/rust.rs", "    #[error(\"Empty argument: {}\", name)]", `    #[error("Guest execution failure: {msg}")]
    Guest { msg: String },
    #[error("Empty argument: {}", name)]`)
	patch(vm, "libwasmvm/src/error/rust.rs", "            VmError::GasDepletion { .. } => RustError::out_of_gas(),", `            VmError::GasDepletion { .. } => RustError::out_of_gas(),
            VmError::Aborted { .. } | VmError::RuntimeErr { .. } | VmError::StaticValidationErr { .. }
            | VmError::CommunicationErr { .. } | VmError::ConversionErr { .. }
            | VmError::CryptoErr { .. } | VmError::ParseErr { .. }
            | VmError::DeserializationLimitExceeded { .. } | VmError::ResolveErr { .. }
            | VmError::ResultMismatch { .. } | VmError::WriteAccessDenied { .. }
            | VmError::MaxCallDepthExceeded { .. } | VmError::FunctionArityMismatch { .. }
                => RustError::Guest { msg: source.to_string() },`)
	patch(vm, "libwasmvm/src/error/rust.rs", "    OutOfGas = 2,", "    OutOfGas = 2,\n    Guest = 3,")
	patch(vm, "libwasmvm/src/error/rust.rs", "        RustError::OutOfGas { .. } => ErrnoValue::OutOfGas,", "        RustError::OutOfGas { .. } => ErrnoValue::OutOfGas,\n        RustError::Guest { .. } => ErrnoValue::Guest,")
	patch(vm, "libwasmvm/src/error/rust.rs", "    fn empty_arg_works() {", `    fn native_async_categories() {
        set_error(RustError::from(VmError::from(BackendError::foreign_panic())), None);
        assert_eq!(errno().0, ErrnoValue::Other as i32);
        set_error(RustError::from(VmError::from(BackendError::out_of_gas())), None);
        assert_eq!(errno().0, ErrnoValue::OutOfGas as i32);
        set_error(RustError::Guest { msg: "trap".into() }, None);
        assert_eq!(errno().0, ErrnoValue::Guest as i32);
    }
    #[test]
    fn empty_arg_works() {`)
	patch(vm, "libwasmvm/src/version.rs", "use std::os::raw::c_char;", `use std::os::raw::c_char;

/// Additive ABI capability; linking stock libraries must fail closed.
#[no_mangle]
pub extern "C" fn native_async_error_abi() -> u32 { 1 }
`)
	patch(vm, "internal/api/version.go", "func LibwasmvmVersion()", `func init() {
 if C.native_async_error_abi() != 1 { panic("native async VM error ABI mismatch") }
}

func LibwasmvmVersion()`)
	patch(vm, "internal/api/lib.go", "\tif msg == nil {\n\t\treturn err\n\t}\n\treturn fmt.Errorf(\"%s\", string(msg))", `
 if errno, ok := err.(syscall.Errno); ok && int(errno) == 3 {
  return types.GuestError{Message: string(msg)}
 }
 // Infrastructure must not become a catchable contract error in wasmd.
 panic(types.InfrastructureError{Message: fmt.Sprintf("native VM error: %v: %s", err, msg)})`)
	typePath := filepath.Join(vm, "types/native_async_error.go")
	if e := os.WriteFile(typePath, []byte(`package types

// GuestError is a deterministically classified guest execution failure.
type GuestError struct { Message string }
func (e GuestError) Error() string { return e.Message }

// InfrastructureError must abort the enclosing block, never charge a task.
type InfrastructureError struct { Message string }
func (e InfrastructureError) Error() string { return e.Message }
`), 0644); e != nil {
		panic(e)
	}
	patch(vm, "lib_libwasmvm.go", `return fmt.Errorf("Insufficient gas left to deserialize contract execution result (%d bytes)", len(data))`, `return types.OutOfGasError{}`)
	patch(vm, "lib_libwasmvm.go", "err := json.Unmarshal(data, response)\n\tif err != nil {\n\t\treturn err\n\t}", "err := json.Unmarshal(data, response)\n\tif err != nil {\n\t\treturn types.GuestError{Message: err.Error()}\n\t}")
	patch(vm, "lib_libwasmvm.go", `return fmt.Errorf("reply contains submessage at index %d with payload larger than %d bytes: %d bytes", i, ReplyPayloadMaxBytes, len(m.Payload))`, `return types.GuestError{Message: fmt.Sprintf("reply contains submessage at index %d with payload larger than %d bytes: %d bytes", i, ReplyPayloadMaxBytes, len(m.Payload))}`)
}

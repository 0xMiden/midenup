use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
struct Stdlib {
    version: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
struct MidenLib {
    version: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
struct Midenc {
    version: String,
}

#[derive(Default, Serialize, Deserialize, Debug)]
struct Toolchain {
    // This is the version that identifies the toolchain itself. Each component
    // from the toolchain will have its own version separately.
    version: String,

    stdlib: Stdlib,
    #[serde(rename(deserialize = "miden-lib"))]
    miden_lib: MidenLib,
    midenc: Midenc,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct Manifest {
    #[serde(rename(deserialize = "manifest-version"))]
    manifest_version: String,
    date: String,
    available_toolchains: Vec<Toolchain>,
}

fn main() {
    let toolchain = Toolchain {
        // Derived from the latest version of the Miden-VM
        version: String::from("0.14.0"),
        stdlib: Stdlib {
            version: String::from("0.15.0"),
        },
        miden_lib: MidenLib {
            version: String::from("0.9.0"),
        },
        midenc: Midenc {
            version: String::from("0.1.0"),
        },
    };

    let manifest = Manifest {
        manifest_version: String::from("1.0"),
        date: String::from("2025/06/06"),
        available_toolchains: vec![toolchain],
    };
    let result = serde_json::to_string(&manifest).unwrap();
    std::println!("{result}");
}

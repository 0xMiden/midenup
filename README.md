# midenup
Warning: This still is in a work in progress state.

Note: `toolchain` currenltly refers to: [`miden-stdlib`, `miden-lib` and `midenc`].

## Functionalities
### init
`midenup init` is the first command that should be run after installation.

Currently, it handles the creation of the `.miden` directory where the different toolchain versions are going to be installed.

### install
`midenup install` handles toolchain installation.

It currently supports one of two arguments
- `stable`: Install the latest available stable releasee
- `<semantic version>`: Install a specific release

Currenltly, this is information is obtained via the `channel-miden.json` file, however in reality this file will be fetched from a Github Page.

## Usage
Here's an example use case:
``` shell
$ ./target/debug/midenup init
$ ./target/debug/midenup install stable
$ tree ~/.miden/
.miden
└── toolchain-0.15.0
    ├── bin
    │   └── midenc
    ├── install
    │   └── install.rs
    └── lib
        ├── miden-lib.masl
        └── std.masl
$ ./target/debug/midenup install 0.14.0
$ tree ~/.miden/
.miden
├── toolchain-0.14.0
│   ├── bin
│   │   └── midenc
│   └── install
│       └── install.rs
└── toolchain-0.15.0
    ├── bin
    │   └── midenc
    ├── install
    │   └── install.rs
    └── lib
        ├── miden-lib.masl
        └── std.masl
```

Each toolchain is installed in a separate directory in the `~/.miden` directory.


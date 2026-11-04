# Building the alignment branch on Windows 11

The build setup can wait until you are ready to test. This fork does not require GitHub Actions builds.

Install Git for Windows, Rust's stable MSVC toolchain (minimum Rust 1.95), and Visual Studio 2022 Build Tools with **Desktop development with C++**, the MSVC x64/x86 tools and a Windows SDK. The C++ workload supplies the Windows linker; the application changes are Rust.

In PowerShell, use a folder where you want to keep the source. Paths with spaces are supported when quoted:

```powershell
git clone https://github.com/U-Type/lightcraft.git
cd lightcraft
git remote add upstream https://github.com/storytold/lightcraft.git
git switch alignment
rustc --version
cargo build --locked --release -p lightcraft
cargo run --locked --release -p lightcraft -- --memory
```

`--memory` starts a disposable demo session. Use it for the first launch. For real imports, launch without `--memory` and use a separate test library initially. The executable is `target\release\lightcraft.exe`. Optional craft-fonts are needed for full Chinese/Japanese glyph coverage; see the upstream AGENTS.md/README instructions.

To update this custom build after a reviewed change:

```powershell
git switch alignment
git pull --ff-only origin alignment
cargo build --locked --release -p lightcraft
```

Keep the custom executable in its own folder. The official updater is disabled in this fork. Use the source branch for updates.

Local focused checks:

```powershell
cargo test --locked -p lightcraft-merge registration
cargo test --locked -p lightcraft-engine --lib alignment
cargo test --locked -p lightcraft-ui-egui --lib alignment
cargo xtask ci
```

Record the first runtime issues with the terminal output, steps, and a screenshot. De-identified example photos will be useful for evaluating automatic landmark suggestions, but synthetic tests and manual alignment do not require them.

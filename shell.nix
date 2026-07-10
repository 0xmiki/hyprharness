{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  packages = with pkgs; [
    # Rust toolchain
    rustc
    cargo
    rustfmt
    clippy

    # Useful development tools
    pkg-config
  ];

  env = {
    RUST_BACKTRACE = "1";
    RUST_LOG = "hyprharness=debug";
  };

  shellHook = ''
    echo "🦀 hyprharness development shell"
    echo "Rust: $(rustc --version)"
    echo "Cargo: $(cargo --version)"
  '';
}
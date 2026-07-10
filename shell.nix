{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  packages = with pkgs; [
    # Rust toolchain
    rustc
    cargo
    rustfmt
    clippy

    # Native Wayland support and screenshot capture
    pkg-config
    wayland
    wayland-protocols
    grim
  ];

  env = {
    RUST_BACKTRACE = "1";
    RUST_LOG = "hyprharness=debug";
  };

  shellHook = ''
    echo "🦀 hyprharness development shell" >&2
    echo "Rust: $(rustc --version)" >&2
    echo "Cargo: $(cargo --version)" >&2
  '';
}

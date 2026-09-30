Source: videotoolbox 0.18.1 from crates.io, with its original MIT / Apache-2.0
licenses. Five macOS-only functions/properties and their public re-exports are
gated to macOS. They otherwise leave undefined iOS symbols in the Rust static
archive even though the controller does not call them. The codec implementation
used by Removent is unchanged. Remove when upstream gates these APIs by platform.

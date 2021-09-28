[![crates.io page](https://img.shields.io/crates/v/srcsrv.svg)](https://crates.io/crates/srcsrv)
[![docs.rs page](https://docs.rs/srcsrv/badge.svg)](https://docs.rs/srcsrv/)

# srcsrv

Parse a `srcsrv` stream from a Windows PDB file and look up file
paths to see how the source for these paths can be obtained:

 - Either by downloading the file from a URL directly ([`SourceRetrievalMethod::Download`](https://docs.rs/srcsrv/0.2.1/srcsrv/enum.SourceRetrievalMethod.html#variant.Download)),
 - or by executing a command, which will create the file at a certain path ([`SourceRetrievalMethod::ExecuteCommand`](https://docs.rs/srcsrv/0.2.1/srcsrv/enum.SourceRetrievalMethod.html#variant.ExecuteCommand))

```rust
use srcsrv::{SrcSrvStream, SourceRetrievalMethod};

if let Ok(srcsrv_stream) = pdb.named_stream(b"srcsrv") {
    let stream = SrcSrvStream::parse(srcsrv_stream.as_slice())?;
    let url = match stream.source_for_path(
        r#"C:\build\renderdoc\renderdoc\data\glsl\gl_texsample.h"#,
        r#"C:\Debugger\Cached Sources"#,
    )? {
        SourceRetrievalMethod::Download { url } => Some(url),
        _ => None,
    };
    assert_eq!(url, Some("https://raw.githubusercontent.com/baldurk/renderdoc/v1.15/renderdoc/data/glsl/gl_texsample.h".to_string()));
}

```

## Microsoft Documentation

 - [Overview](https://docs.microsoft.com/en-us/windows/win32/debug/source-server-and-source-indexing)
 - [Language specification](https://docs.microsoft.com/en-us/windows-hardware/drivers/debugger/language-specification-1)

## License

Licensed under either of

  * Apache License, Version 2.0 ([`LICENSE-APACHE`](./LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
  * MIT license ([`LICENSE-MIT`](./LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

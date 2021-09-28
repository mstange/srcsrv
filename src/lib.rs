//! Parse a `srcsrv` stream from a Windows PDB file and look up file
//! paths to see how the source for these paths can be obtained:
//!
//!  - Either by downloading the file from a URL directly ([`SourceRetrievalMethod::Download`]),
//!  - or by executing a command, which will create the file at a certain path ([`SourceRetrievalMethod::ExecuteCommand`])
//!
//! ```
//! use srcsrv::{SrcSrvStream, SourceRetrievalMethod};
//!
//! # fn wrapper<'s, S: pdb::Source<'s> + 's>(pdb: &mut pdb::PDB<'s, S>) -> std::result::Result<(), Box<dyn std::error::Error>> {
//! if let Ok(srcsrv_stream) = pdb.named_stream(b"srcsrv") {
//!     let stream = SrcSrvStream::parse(srcsrv_stream.as_slice())?;
//!     let url = match stream.source_for_path(
//!         r#"C:\build\renderdoc\renderdoc\data\glsl\gl_texsample.h"#,
//!         r#"C:\Debugger\Cached Sources"#,
//!     )? {
//!         SourceRetrievalMethod::Download { url } => Some(url),
//!         _ => None,
//!     };
//!     assert_eq!(url, Some("https://raw.githubusercontent.com/baldurk/renderdoc/v1.15/renderdoc/data/glsl/gl_texsample.h".to_string()));
//! }
//! # Ok(())
//! # }
//! ```

use std::collections::{HashMap, HashSet};
use std::result::Result;

mod ast;
mod errors;

use ast::AstNode;
pub use errors::{EvalError, ParseError};

/// A map of variables with their evaluated values.
pub type EvalVarMap = HashMap<String, String>;

/// Describes how the source file can be obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceRetrievalMethod {
    /// The source can be downloaded from the web, at the given URL.
    Download { url: String },
    /// Evaluating the given command on the Windows Command shell with the given
    /// environment variables will create the source file at `target_path`.
    ExecuteCommand {
        /// The command to execute.
        command: String,
        /// The environment veriables to set during command execution.
        env: HashMap<String, String>,
        /// An optional version control string.
        version_ctrl: Option<String>,
        /// The path at which the extracted file will appear once the command has run.
        target_path: String,
        /// An optional string which identifies files that use the same version control
        /// system. Used for error persistence.
        /// If a file encounters an error during command execution, and the command output
        /// matches one of the strings in [`SrcSrvStream::error_persistence_command_output_strings()`],
        /// execution of the command should be skipped for all future entries with the same
        /// `error_persistence_version_control` value.
        /// See <https://docs.microsoft.com/en-us/windows-hardware/drivers/debugger/language-specification-1#handling-server-errors>.
        error_persistence_version_control: Option<String>,
    },
    /// Grab bag for other cases. Please file issues about any extra cases you need.
    Other { raw_var_values: EvalVarMap },
}

/// A parsed representation of the `srcsrv` stream from a PDB file.
pub struct SrcSrvStream<'a> {
    /// 1, 2 or 3, based on the VERSION={} field
    version: u8,
    /// lowercase field name -> field value
    ini_fields: HashMap<String, &'a str>,
    /// lowercase field name -> (raw field value, parsed field value ast node)
    var_fields: HashMap<String, (&'a str, AstNode<'a>)>,
    /// lowercase original path -> [var1, ..., var10]
    source_file_entries: HashMap<String, Vec<&'a str>>,
}

impl<'a> SrcSrvStream<'a> {
    /// Parse the `srcsrv` stream. The stream bytes can be obtained with the help of
    /// the [`PDB::named_stream` method from the `pdb` crate](https://docs.rs/pdb/0.7.0/pdb/struct.PDB.html#method.named_stream).
    ///
    /// ```
    /// use srcsrv::SrcSrvStream;
    ///
    /// # fn wrapper<'s, S: pdb::Source<'s> + 's>(pdb: &mut pdb::PDB<'s, S>) -> std::result::Result<(), srcsrv::ParseError> {
    /// if let Ok(srcsrv_stream) = pdb.named_stream(b"srcsrv") {
    ///     let stream = SrcSrvStream::parse(srcsrv_stream.as_slice())?;
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn parse(stream: &'a [u8]) -> Result<SrcSrvStream<'a>, ParseError> {
        let stream = std::str::from_utf8(stream).map_err(|_| ParseError::InvalidUtf8)?;
        let mut lines = stream.lines();

        // Parse section SRCSRV: ini ------------------------------------------------
        let first_line = lines.next().ok_or(ParseError::UnexpectedEof)?;
        if !first_line.starts_with("SRCSRV: ini --") {
            return Err(ParseError::MissingIniSection);
        }

        let mut ini_fields = HashMap::new();
        let next_section_start_line = loop {
            let line = lines.next().ok_or(ParseError::UnexpectedEof)?;
            if line.starts_with("SRCSRV:") {
                break line;
            }

            let (name, value) = line.split_once('=').ok_or(ParseError::MissingEquals)?;
            ini_fields.insert(name.to_ascii_lowercase(), value);
        };

        let version = match ini_fields.get(&"VERSION".to_ascii_lowercase()) {
            Some(&"1") => 1,
            Some(&"2") => 2,
            Some(&"3") => 3,
            Some(v) => return Err(ParseError::UnrecognizedVersion(v.to_string())),
            None => return Err(ParseError::MissingVersion),
        };

        // Parse section SRCSRV: variables ------------------------------------------
        if !next_section_start_line.starts_with("SRCSRV: variables --") {
            return Err(ParseError::MissingVariablesSection);
        }

        let mut var_fields = HashMap::new();
        let next_section_start_line = loop {
            let line = lines.next().ok_or(ParseError::UnexpectedEof)?;
            if line.starts_with("SRCSRV:") {
                break line;
            }

            let (name, value) = line.split_once('=').ok_or(ParseError::MissingEquals)?;
            let node = AstNode::try_from_str(value)?;
            var_fields.insert(name.to_ascii_lowercase(), (value, node));
        };

        if !var_fields.contains_key(&"SRCSRVTRG".to_ascii_lowercase()) {
            return Err(ParseError::MissingSrcSrvTrgField);
        }

        // Parse section SRCSRV: source files ---------------------------------------
        if !next_section_start_line.starts_with("SRCSRV: source files --") {
            return Err(ParseError::MissingSourceFilesSection);
        }

        let mut source_file_entries = HashMap::new();
        let end_line = loop {
            let line = lines.next().ok_or(ParseError::UnexpectedEof)?;
            if line.starts_with("SRCSRV:") {
                break line;
            }

            let vars: Vec<&str> = line.splitn(10, '*').collect();
            source_file_entries.insert(vars[0].to_ascii_lowercase(), vars);
        };

        // Stop at SRCSRV: end ------------------------------------------------
        if !end_line.starts_with("SRCSRV: end --") {
            return Err(ParseError::MissingTerminationLine);
        }

        Ok(SrcSrvStream {
            version,
            ini_fields,
            var_fields,
            source_file_entries,
        })
    }

    /// The value of the VERSION field from the ini section.
    pub fn version(&self) -> u8 {
        self.version
    }

    /// The value of the INDEXVERSION field from the ini section, if specified.
    pub fn index_version(&self) -> Option<&'a str> {
        self.ini_fields.get("indexversion").cloned()
    }

    /// The value of the DATETIME field from the ini section, if specified.
    pub fn datetime(&self) -> Option<&'a str> {
        self.ini_fields.get("datetime").cloned()
    }

    /// The value of the VERCTRL field from the ini section, if specified.
    pub fn version_control_description(&self) -> Option<&'a str> {
        self.ini_fields.get("verctrl").cloned()
    }

    /// Look up `original_file_path` in the file entries and find out how to obtain
    /// the source for this file. This evaluates the variables for the matching file
    /// entry.
    ///
    /// `extraction_base_path` is used as the value of the special `%targ%` variable
    /// and should not include a trailing backslash.
    ///
    /// Returns `Ok(None)` if the file path was not found in the list of file entries.
    ///
    /// ```
    /// use srcsrv::{SrcSrvStream, SourceRetrievalMethod};
    ///
    /// # fn wrapper() -> std::result::Result<(), Box<dyn std::error::Error>> {
    /// # let stream = SrcSrvStream::parse(&[])?;
    /// println!(
    ///     "{:#?}",
    ///     stream.source_for_path(
    ///         r#"C:\build\renderdoc\renderdoc\data\glsl\gl_texsample.h"#,
    ///         r#"C:\Debugger\Cached Sources"#
    ///     )?
    /// );
    /// # Ok(())
    /// # }
    /// ```
    pub fn source_for_path(
        &self,
        original_file_path: &str,
        extraction_base_path: &str,
    ) -> Result<Option<SourceRetrievalMethod>, EvalError> {
        match self.source_and_raw_var_values_for_path(original_file_path, extraction_base_path)? {
            Some((method, _)) => Ok(Some(method)),
            None => Ok(None),
        }
    }

    /// Look up `original_file_path` in the file entries and find out how to obtain
    /// the source for this file. This evaluates the variables for the matching file
    /// entry.
    ///
    /// `extraction_base_path` is used as the value of the special `%targ%` variable
    /// and should not include a trailing backslash.
    ///
    /// This method additionally returns the raw values of all variables. This gives
    /// consumers more ways to special-case their behavior. It also acts as an escape
    /// hatch if there are any cases that `SourceRetrievalMethod` does not cover.
    /// If you don't need the raw variable values, prefer to call `source_for_path`
    /// instead.
    ///
    /// Returns `Ok(None)` if the file path was not found in the list of file entries.
    pub fn source_and_raw_var_values_for_path(
        &self,
        original_file_path: &str,
        extraction_base_path: &str,
    ) -> Result<Option<(SourceRetrievalMethod, EvalVarMap)>, EvalError> {
        let error_persistence_version_control_var = self.get_raw_var("SRCSRVERRVAR");
        let mut map = EvalVarMap::new();
        let found = self.add_vars_for_file(original_file_path, &mut map)?;
        if !found {
            return Ok(None);
        }

        let error_persistence_version_control =
            error_persistence_version_control_var.and_then(|var| map.get(var).cloned());

        map.insert("targ".to_string(), extraction_base_path.to_string());

        let target = self.evaluate_required_field("SRCSRVTRG", &mut map)?;
        let command = self.evaluate_optional_field("SRCSRVCMD", &mut map)?;
        let env = self.evaluate_optional_field("SRCSRVENV", &mut map)?;
        let version_ctrl = self.evaluate_optional_field("SRCSRVVERCTRL", &mut map)?;

        if let Some(command) = command {
            let env = match env {
                Some(env) => env
                    .split('\x08')
                    .filter_map(|s| s.split_once('='))
                    .map(|(envname, envval)| (envname.to_owned(), envval.to_owned()))
                    .collect(),
                None => HashMap::new(),
            };
            return Ok(Some((
                SourceRetrievalMethod::ExecuteCommand {
                    command,
                    env,
                    target_path: target,
                    version_ctrl,
                    error_persistence_version_control,
                },
                map,
            )));
        }

        if target.starts_with("http://") || target.starts_with("https://") {
            return Ok(Some((SourceRetrievalMethod::Download { url: target }, map)));
        }

        Ok(Some((
            SourceRetrievalMethod::Other {
                raw_var_values: map.clone(),
            },
            map,
        )))
    }

    /// A set of strings which can be substring-matched to the output of the
    /// command that executed when obtaining source files.
    ///
    /// If any of the strings matches, it is recommended to "persist the error"
    /// and refuse to execute further commands for other files with the same
    /// `error_persistence_version_control` value.
    pub fn error_persistence_command_output_strings(&self) -> HashSet<&'a str> {
        self.var_fields
            .iter()
            .filter_map(|(var_name, (var_value, _))| {
                if var_name.starts_with(&"SRCSRVERRDESC".to_ascii_lowercase()) {
                    Some(*var_value)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get the value of the specified field from the ini section.
    /// The field name is case-insensitive.
    pub fn get_ini_field(&self, field_name: &str) -> Option<&'a str> {
        self.ini_fields
            .get(&field_name.to_ascii_lowercase())
            .cloned()
    }

    /// Get the raw, unevaluated value of the specified field from the
    /// variables section.
    /// The field name is case-insensitive.
    pub fn get_raw_var(&self, var_name: &str) -> Option<&'a str> {
        self.var_fields
            .get(&var_name.to_ascii_lowercase())
            .map(|(val, _)| *val)
    }

    /// Add the values of var1, ..., var10 to the map, for the given file path.
    /// Returns Ok(false) if the file was not found.
    fn add_vars_for_file(&self, file_path: &str, map: &mut EvalVarMap) -> Result<bool, EvalError> {
        let vars = match self
            .source_file_entries
            .get(&file_path.to_ascii_lowercase())
        {
            Some(vars) => vars,
            None => return Ok(false),
        };

        map.extend(
            vars.iter()
                .enumerate()
                .map(|(i, var)| (format!("var{}", i + 1), var.to_string())),
        );

        Ok(true)
    }

    fn evaluate_optional_field(
        &self,
        var_name: &str,
        var_map: &mut EvalVarMap,
    ) -> Result<Option<String>, EvalError> {
        let var_name = var_name.to_ascii_lowercase();
        if !self.var_fields.contains_key(&var_name) {
            return Ok(None);
        }
        let val = self.eval_impl(var_name, var_map, &mut vec![])?;
        Ok(Some(val))
    }

    fn evaluate_required_field(
        &self,
        var_name: &str,
        var_map: &mut EvalVarMap,
    ) -> Result<String, EvalError> {
        let var_name = var_name.to_ascii_lowercase();
        self.eval_impl(var_name, var_map, &mut vec![])
    }

    fn eval_impl(
        &self,
        var_name: String,
        var_map: &mut EvalVarMap,
        eval_stack: &mut Vec<String>,
    ) -> Result<String, EvalError> {
        if let Some(val) = var_map.get(&var_name) {
            return Ok(val.clone());
        }
        if eval_stack.contains(&var_name) {
            return Err(EvalError::Recursion(var_name));
        }

        eval_stack.push(var_name.clone());

        let node = match self.var_fields.get(&var_name) {
            Some((_, node)) => node,
            None => return Err(EvalError::UnknownVariable(var_name)),
        };
        let mut get_var =
            |var_name: &str| self.eval_impl(var_name.to_ascii_lowercase(), var_map, eval_stack);
        let eval_val = node.eval(&mut get_var)?;
        var_map.insert(var_name, eval_val.clone());

        eval_stack.pop();

        Ok(eval_val)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::{SourceRetrievalMethod, SrcSrvStream};

    #[test]
    fn firefox() {
        let stream = r#"SRCSRV: ini ------------------------------------------------
VERSION=2
INDEXVERSION=2
VERCTRL=http
SRCSRV: variables ------------------------------------------
HGSERVER=https://hg.mozilla.org/mozilla-central
SRCSRVVERCTRL=http
HTTP_EXTRACT_TARGET=%hgserver%/raw-file/%var3%/%var2%
SRCSRVTRG=%http_extract_target%
SRCSRV: source files ---------------------------------------
/builds/worker/checkouts/gecko/mozglue/build/SSE.cpp*mozglue/build/SSE.cpp*1706d4d54ec68fae1280305b70a02cb24c16ff68
/builds/worker/checkouts/gecko/memory/build/mozjemalloc.cpp*memory/build/mozjemalloc.cpp*1706d4d54ec68fae1280305b70a02cb24c16ff68
/builds/worker/checkouts/gecko/vs2017_15.8.4/VC/include/algorithm*vs2017_15.8.4/VC/include/algorithm*1706d4d54ec68fae1280305b70a02cb24c16ff68
/builds/worker/checkouts/gecko/mozglue/baseprofiler/core/ProfilerBacktrace.cpp*mozglue/baseprofiler/core/ProfilerBacktrace.cpp*1706d4d54ec68fae1280305b70a02cb24c16ff68
/builds/worker/workspace/obj-build/dist/include/mozilla/IntegerRange.h*mfbt/IntegerRange.h*1706d4d54ec68fae1280305b70a02cb24c16ff68
SRCSRV: end ------------------------------------------------


"#;
        let stream = SrcSrvStream::parse(stream.as_bytes()).unwrap();
        assert_eq!(stream.version(), 2);
        assert_eq!(stream.datetime(), None);
        assert_eq!(stream.version_control_description(), Some("http"));
        assert_eq!(
            stream
                .source_for_path(
                    r#"/builds/worker/checkouts/gecko/mozglue/baseprofiler/core/ProfilerBacktrace.cpp"#,
                    r#"C:\Debugger\Cached Sources"#
                )
                .unwrap().unwrap(),
            SourceRetrievalMethod::Download {
                url: "https://hg.mozilla.org/mozilla-central/raw-file/1706d4d54ec68fae1280305b70a02cb24c16ff68/mozglue/baseprofiler/core/ProfilerBacktrace.cpp".to_string()
            }
        );
    }

    #[test]
    fn chrome() {
        // From https://chromium-browser-symsrv.commondatastorage.googleapis.com/chrome.dll.pdb/5D664C4A228FA9804C4C44205044422E1/chrome.dll.pdb
        let stream = r#"SRCSRV: ini ------------------------------------------------
VERSION=1
INDEXVERSION=2
VERCTRL=Subversion
DATETIME=Fri Jul 30 14:11:46 2021
SRCSRV: variables ------------------------------------------
SRC_EXTRACT_TARGET_DIR=%targ%\%fnbksl%(%var2%)\%var3%
SRC_EXTRACT_TARGET=%SRC_EXTRACT_TARGET_DIR%\%fnfile%(%var1%)
SRC_EXTRACT_CMD=cmd /c "mkdir "%SRC_EXTRACT_TARGET_DIR%" & python -c "import urllib2, base64;url = \"%var4%\";u = urllib2.urlopen(url);open(r\"%SRC_EXTRACT_TARGET%\", \"wb\").write(%var5%(u.read()))"
SRCSRVTRG=%SRC_EXTRACT_TARGET%
SRCSRVCMD=%SRC_EXTRACT_CMD%
SRCSRV: source files ---------------------------------------
c:\b\s\w\ir\cache\builder\src\third_party\pdfium\core\fdrm\fx_crypt.cpp*core/fdrm/fx_crypt.cpp*dab1161c861cc239e48a17e1a5d729aa12785a53*https://pdfium.googlesource.com/pdfium.git/+/dab1161c861cc239e48a17e1a5d729aa12785a53/core/fdrm/fx_crypt.cpp?format=TEXT*base64.b64decode
c:\b\s\w\ir\cache\builder\src\third_party\pdfium\core\fdrm\fx_crypt_aes.cpp*core/fdrm/fx_crypt_aes.cpp*dab1161c861cc239e48a17e1a5d729aa12785a53*https://pdfium.googlesource.com/pdfium.git/+/dab1161c861cc239e48a17e1a5d729aa12785a53/core/fdrm/fx_crypt_aes.cpp?format=TEXT*base64.b64decode
SRCSRV: end ------------------------------------------------"#;
        let stream = SrcSrvStream::parse(stream.as_bytes()).unwrap();
        assert_eq!(stream.version(), 1);
        assert_eq!(stream.datetime(), Some("Fri Jul 30 14:11:46 2021"));
        assert_eq!(stream.version_control_description(), Some("Subversion"));
        assert_eq!(
            stream
                .source_for_path(
                    r#"c:\b\s\w\ir\cache\builder\src\third_party\pdfium\core\fdrm\fx_crypt.cpp"#,
                    r#"C:\Debugger\Cached Sources"#,
                )
                .unwrap().unwrap(),
            SourceRetrievalMethod::ExecuteCommand {
                command: r#"cmd /c "mkdir "C:\Debugger\Cached Sources\core\fdrm\fx_crypt.cpp\dab1161c861cc239e48a17e1a5d729aa12785a53" & python -c "import urllib2, base64;url = \"https://pdfium.googlesource.com/pdfium.git/+/dab1161c861cc239e48a17e1a5d729aa12785a53/core/fdrm/fx_crypt.cpp?format=TEXT\";u = urllib2.urlopen(url);open(r\"C:\Debugger\Cached Sources\core\fdrm\fx_crypt.cpp\dab1161c861cc239e48a17e1a5d729aa12785a53\fx_crypt.cpp\", \"wb\").write(base64.b64decode(u.read()))""#.to_string(),
                env: HashMap::new(),
                target_path: r#"C:\Debugger\Cached Sources\core\fdrm\fx_crypt.cpp\dab1161c861cc239e48a17e1a5d729aa12785a53\fx_crypt.cpp"#.to_string(),
                version_ctrl: None,
                error_persistence_version_control: None,
            }
        );
    }

    #[test]
    fn team_foundation() {
        // From https://github.com/microsoft/perfview/blob/5c9f6059f54db41b4ac5c4fc8f57261779634489/src/TraceEvent/Symbols/NativeSymbolModule.cs#L776
        let stream = r#"SRCSRV: ini ------------------------------------------------
VERSION=3
INDEXVERSION=2
VERCTRL=Team Foundation Server
DATETIME=Thu Mar 10 16:15:55 2016
SRCSRV: variables ------------------------------------------
TFS_EXTRACT_CMD=tf.exe view /version:%var4% /noprompt "$%var3%" /server:%fnvar%(%var2%) /output:%srcsrvtrg%
TFS_EXTRACT_TARGET=%targ%\%var2%%fnbksl%(%var3%)\%var4%\%fnfile%(%var1%)
VSTFDEVDIV_DEVDIV2=http://vstfdevdiv.redmond.corp.microsoft.com:8080/DevDiv2
SRCSRVVERCTRL=tfs
SRCSRVERRDESC=access
SRCSRVERRVAR=var2
SRCSRVTRG=%TFS_extract_target%
SRCSRVCMD=%TFS_extract_cmd%
SRCSRV: source files ---------------------------------------
f:\dd\externalapis\legacy\vctools\vc12\inc\cvconst.h*VSTFDEVDIV_DEVDIV2*/DevDiv/Fx/Rel/NetFxRel3Stage/externalapis/legacy/vctools/vc12/inc/cvconst.h*1363200
f:\dd\externalapis\legacy\vctools\vc12\inc\cvinfo.h*VSTFDEVDIV_DEVDIV2*/DevDiv/Fx/Rel/NetFxRel3Stage/externalapis/legacy/vctools/vc12/inc/cvinfo.h*1363200
f:\dd\externalapis\legacy\vctools\vc12\inc\vc\ammintrin.h*VSTFDEVDIV_DEVDIV2*/DevDiv/Fx/Rel/NetFxRel3Stage/externalapis/legacy/vctools/vc12/inc/vc/ammintrin.h*1363200
SRCSRV: end ------------------------------------------------"#;
        let stream = SrcSrvStream::parse(stream.as_bytes()).unwrap();
        assert_eq!(stream.version(), 3);
        assert_eq!(stream.datetime(), Some("Thu Mar 10 16:15:55 2016"));
        assert_eq!(
            stream.version_control_description(),
            Some("Team Foundation Server")
        );
        assert_eq!(
            stream
                .source_for_path(
                    r#"F:\dd\externalapis\legacy\vctools\vc12\inc\cvinfo.h"#,
                    r#"C:\Debugger\Cached Sources"#,
                )
                .unwrap().unwrap(),
                SourceRetrievalMethod::ExecuteCommand {
                    command: r#"tf.exe view /version:1363200 /noprompt "$/DevDiv/Fx/Rel/NetFxRel3Stage/externalapis/legacy/vctools/vc12/inc/cvinfo.h" /server:http://vstfdevdiv.redmond.corp.microsoft.com:8080/DevDiv2 /output:C:\Debugger\Cached Sources\VSTFDEVDIV_DEVDIV2\DevDiv\Fx\Rel\NetFxRel3Stage\externalapis\legacy\vctools\vc12\inc\cvinfo.h\1363200\cvinfo.h"#.to_string(),
                    env: HashMap::new(),
                    version_ctrl: Some("tfs".to_string()),
                    target_path: r#"C:\Debugger\Cached Sources\VSTFDEVDIV_DEVDIV2\DevDiv\Fx\Rel\NetFxRel3Stage\externalapis\legacy\vctools\vc12\inc\cvinfo.h\1363200\cvinfo.h"#.to_string(),
                    error_persistence_version_control: Some("VSTFDEVDIV_DEVDIV2".to_string()),
                }
        );
    }

    #[test]
    fn renderdoc() {
        // From https://renderdoc.org/symbols/renderdoc.pdb/6D1DFFC4DC524537962CCABC000820641/renderdoc.pd_
        let stream = r#"SRCSRV: ini ------------------------------------------------
VERSION=2
VERCTRL=http
SRCSRV: variables ------------------------------------------
HTTP_ALIAS=https://raw.githubusercontent.com/baldurk/renderdoc/v1.15/
HTTP_EXTRACT_TARGET=%HTTP_ALIAS%%var2%
SRCSRVTRG=%HTTP_EXTRACT_TARGET%
SRCSRV: source files ---------------------------------------
C:\build\renderdoc\qrenderdoc\Code\BufferFormatter.cpp*qrenderdoc/Code/BufferFormatter.cpp
C:\build\renderdoc\qrenderdoc\Windows\Dialogs\AnalyticsConfirmDialog.cpp*qrenderdoc/Windows/Dialogs/AnalyticsConfirmDialog.cpp
C:\build\renderdoc\renderdoc\data\glsl\gl_texsample.h*renderdoc/data/glsl/gl_texsample.h
C:\build\renderdoc\renderdoc\driver\d3d12\d3d12_device.cpp*renderdoc/driver/d3d12/d3d12_device.cpp
C:\build\renderdoc\renderdoc\maths\matrix.cpp*renderdoc/maths/matrix.cpp
C:\build\renderdoc\util\test\demos\texture_zoo.cpp*util/test/demos/texture_zoo.cpp
C:\build\renderdoc\Win32\Release\renderdoc_app.h*Win32/Release/renderdoc_app.h
C:\build\renderdoc\x64\Release\renderdoc_app.h*x64/Release/renderdoc_app.h
SRCSRV: end ------------------------------------------------"#;
        let stream = SrcSrvStream::parse(stream.as_bytes()).unwrap();
        assert_eq!(stream.version(), 2);
        assert_eq!(stream.datetime(), None);
        assert_eq!(stream.version_control_description(), Some("http"));
        assert_eq!(
            stream
                .source_for_path(
                    r#"C:\build\renderdoc\renderdoc\data\glsl\gl_texsample.h"#,
                    r#"C:\Debugger\Cached Sources"#,
                )
                .unwrap().unwrap(),
                SourceRetrievalMethod::Download {
                    url: "https://raw.githubusercontent.com/baldurk/renderdoc/v1.15/renderdoc/data/glsl/gl_texsample.h".to_string(),
                }
        );
    }
}

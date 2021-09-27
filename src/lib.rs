use std::collections::HashMap;
use std::result::Result;

mod ast;
mod errors;

pub use ast::AstNode;
pub use errors::{EvalError, ParseError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HowToObtainSource {
    FromHttp {
        url: String,
    },
    ByExecutingCommand {
        command: String,
        env: HashMap<String, String>,
        target_path: String,
    },
    Other {
        version_ctrl: Option<String>,
        target_path: String,
    },
}

pub struct SrcSrvStream<'a> {
    /// 1, 2 or 3, based on the VERSION={} field
    version: u8,
    /// lowercase field name -> field value
    ini_fields: HashMap<String, &'a str>,
    /// lowercase field name -> field value
    var_fields: HashMap<String, (&'a str, AstNode<'a>)>,
    /// lowercase original path -> [var1, ..., var10]
    source_file_entries: HashMap<String, Vec<&'a str>>,
}

impl<'a> SrcSrvStream<'a> {
    pub fn new_from_slice(stream: &'a [u8]) -> Result<SrcSrvStream<'a>, ParseError> {
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
            Some(_) => return Err(ParseError::UnrecognizedVersion),
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

    pub fn version(&self) -> u8 {
        self.version
    }

    pub fn datetime(&self) -> Option<&'a str> {
        self.ini_fields.get("datetime").cloned()
    }

    pub fn version_control_description(&self) -> Option<&'a str> {
        self.ini_fields.get("verctrl").cloned()
    }

    pub fn get_ini_field(&self, field_name: &str) -> Option<&'a str> {
        self.ini_fields
            .get(&field_name.to_ascii_lowercase())
            .cloned()
    }

    pub fn get_raw_var(&self, var_name: &str) -> Option<&'a str> {
        self.var_fields
            .get(&var_name.to_ascii_lowercase())
            .map(|(val, _)| *val)
    }

    pub fn add_vars_for_file(
        &self,
        file_path: &str,
        map: &mut HashMap<String, String>,
    ) -> Option<()> {
        let vars = self
            .source_file_entries
            .get(&file_path.to_ascii_lowercase())?;

        map.extend(
            [
                "var1", "var2", "var3", "var4", "var5", "var6", "var7", "var8", "var9", "var10",
            ]
            .iter()
            .zip(vars.iter())
            .map(|(k, v)| (k.to_string(), v.to_string())),
        );

        Some(())
    }

    pub fn interpreter_with_base_path(&self, base_path: &str) -> SrcSrvInterpreter<'_> {
        SrcSrvInterpreter {
            stream: self,
            base_path: base_path.to_owned(),
        }
    }
}

pub struct SrcSrvInterpreter<'a> {
    stream: &'a SrcSrvStream<'a>,

    /// The base directory for extracted files, used as %targ%.
    base_path: String,
}

impl<'a> SrcSrvInterpreter<'a> {
    pub fn source_for_path(&self, file_path: &str) -> Result<HowToObtainSource, EvalError> {
        let mut map = HashMap::new();
        map.insert("targ".to_string(), self.base_path.clone());
        self.stream.add_vars_for_file(file_path, &mut map);
        let target = self.evaluate_required_field("SRCSRVTRG", &mut map)?;
        let ver_ctrl = self.evaluate_optional_field("SRCSRVVERCTRL", &mut map)?;
        if ver_ctrl.as_deref() == Some("http") {
            return Ok(HowToObtainSource::FromHttp { url: target });
        }

        let command = self.evaluate_optional_field("SRCSRVCMD", &mut map)?;
        let env = self.evaluate_optional_field("SRCSRVENV", &mut map)?;
        if let Some(command) = command {
            let env = match env {
                Some(env) => env
                    .split('\x08')
                    .filter_map(|s| s.split_once('='))
                    .map(|(envname, envval)| (envname.to_owned(), envval.to_owned()))
                    .collect(),
                None => HashMap::new(),
            };
            return Ok(HowToObtainSource::ByExecutingCommand {
                command,
                env,
                target_path: target,
            });
        }

        Ok(HowToObtainSource::Other {
            version_ctrl: ver_ctrl,
            target_path: target,
        })
    }

    pub fn evaluate_optional_field(
        &self,
        var_name: &str,
        var_map: &mut HashMap<String, String>,
    ) -> Result<Option<String>, EvalError> {
        let var_name = var_name.to_ascii_lowercase();
        if !self.stream.var_fields.contains_key(&var_name) {
            return Ok(None);
        }
        let val = self.eval_impl(var_name, var_map, &mut vec![])?;
        Ok(Some(val))
    }

    pub fn evaluate_required_field(
        &self,
        var_name: &str,
        var_map: &mut HashMap<String, String>,
    ) -> Result<String, EvalError> {
        let var_name = var_name.to_ascii_lowercase();
        self.eval_impl(var_name, var_map, &mut vec![])
    }

    fn eval_impl(
        &self,
        var_name: String,
        var_map: &mut HashMap<String, String>,
        eval_stack: &mut Vec<String>,
    ) -> Result<String, EvalError> {
        if let Some(val) = var_map.get(&var_name) {
            return Ok(val.clone());
        }
        if eval_stack.contains(&var_name) {
            return Err(EvalError::Recursion);
        }

        eval_stack.push(var_name.clone());

        let (_, node) = self
            .stream
            .var_fields
            .get(&var_name)
            .ok_or(EvalError::UnknownVariable)?;
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

    use crate::{HowToObtainSource, SrcSrvStream};

    #[test]
    fn firefox() {
        let stream = r#"SRCSRV: ini ------------------------------------------------
VERSION=2
INDEXVERSION=2
VERCTRL=http
SRCSRV: variables ------------------------------------------
HGSERVER=https://hg.mozilla.org/mozilla-central/
SRCSRVVERCTRL=http
HTTP_EXTRACT_TARGET=%hgserver%/raw-file/%var3%/%var2%
SRCSRVTRG=%http_extract_target%
SRCSRV: source files ---------------------------------------
D:\build\...\Interpreter.cpp*js/src/vm/Interpreter.cpp*24938c537a55f9db3913072d33b178b210e7d6b5
SRCSRV: end ------------------------------------------------"#;
        let stream = SrcSrvStream::new_from_slice(stream.as_bytes()).unwrap();
        assert_eq!(stream.version(), 2);
        assert_eq!(stream.datetime(), None);
        assert_eq!(stream.version_control_description(), Some("http"));
        let interpreter = stream.interpreter_with_base_path(r#"C:\Debugger\Cached Sources"#);
        assert_eq!(
            interpreter
                .source_for_path(
                    r#"D:\build\...\Interpreter.cpp"#
                )
                .unwrap(),
            HowToObtainSource::FromHttp {
                url: "https://hg.mozilla.org/mozilla-central//raw-file/24938c537a55f9db3913072d33b178b210e7d6b5/js/src/vm/Interpreter.cpp".to_string()
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
        let stream = SrcSrvStream::new_from_slice(stream.as_bytes()).unwrap();
        assert_eq!(stream.version(), 1);
        assert_eq!(stream.datetime(), Some("Fri Jul 30 14:11:46 2021"));
        assert_eq!(stream.version_control_description(), Some("Subversion"));
        let interpreter = stream.interpreter_with_base_path(r#"C:\Debugger\Cached Sources"#);
        assert_eq!(
            interpreter
                .source_for_path(
                    r#"c:\b\s\w\ir\cache\builder\src\third_party\pdfium\core\fdrm\fx_crypt.cpp"#
                )
                .unwrap(),
            HowToObtainSource::ByExecutingCommand {
                command: r#"cmd /c "mkdir "C:\Debugger\Cached Sources\core\fdrm\fx_crypt.cpp\dab1161c861cc239e48a17e1a5d729aa12785a53" & python -c "import urllib2, base64;url = \"https://pdfium.googlesource.com/pdfium.git/+/dab1161c861cc239e48a17e1a5d729aa12785a53/core/fdrm/fx_crypt.cpp?format=TEXT\";u = urllib2.urlopen(url);open(r\"C:\Debugger\Cached Sources\core\fdrm\fx_crypt.cpp\dab1161c861cc239e48a17e1a5d729aa12785a53\fx_crypt.cpp\", \"wb\").write(base64.b64decode(u.read()))""#.to_string(),
                env: HashMap::new(),
                target_path: r#"C:\Debugger\Cached Sources\core\fdrm\fx_crypt.cpp\dab1161c861cc239e48a17e1a5d729aa12785a53\fx_crypt.cpp"#.to_string()
            }
        );
    }
}

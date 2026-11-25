//! Generates the OpenAPI document of a Quarkus REST application from its Java sources, byte-identical to what
//! quarkus-smallrye-openapi stores with `quarkus.smallrye-openapi.store-schema-directory`.

mod index;
mod java;
mod model;
mod quarkus;
mod scan;
mod writer;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use model::{MK, V};

/// JDK types that SmallRye can turn into `$ref` components, but only when they are in the Jandex index.
const REGISTRABLE_JDK_TYPES: &[&str] = &[
    "java.util.UUID",
    "java.util.Date",
    "java.sql.Date",
    "java.time.LocalDate",
    "java.time.LocalDateTime",
    "java.time.LocalTime",
    "java.time.OffsetDateTime",
    "java.time.OffsetTime",
    "java.time.ZonedDateTime",
    "java.time.Instant",
    "java.time.Duration",
    "java.time.Period",
];

/// Annotations that make Quarkus (ArC, RESTEasy Reactive, Hibernate ORM) inspect a class' members.
const BEAN_ANNOTATIONS: &[&str] = &[
    "jakarta.enterprise.context.ApplicationScoped",
    "jakarta.enterprise.context.RequestScoped",
    "jakarta.enterprise.context.SessionScoped",
    "jakarta.enterprise.context.Dependent",
    "jakarta.inject.Singleton",
    "jakarta.ws.rs.Path",
    "jakarta.persistence.Entity",
    "jakarta.persistence.Embeddable",
    "jakarta.persistence.MappedSuperclass",
];

/// JDK types that end up in Quarkus' index: Quarkus indexes the types of fields and method signatures of beans,
/// resources and entities when it processes them.
fn indexed_jdk_types(idx: &index::Index) -> Vec<String> {
    let mut found = std::collections::BTreeSet::new();
    let mut visit = |t: &index::Type| {
        let name = t.name();
        if REGISTRABLE_JDK_TYPES.contains(&name.as_str()) {
            found.insert(name);
        }
    };
    for c in &idx.classes {
        if !c
            .annotations
            .iter()
            .any(|a| BEAN_ANNOTATIONS.contains(&a.name.as_str()))
        {
            continue;
        }
        for f in &c.fields {
            visit(&f.ty);
        }
        for m in &c.methods {
            visit(&m.return_type);
            for p in &m.params {
                visit(&p.ty);
            }
        }
    }
    found.into_iter().collect()
}

struct Args {
    project: PathBuf,
    out: Option<PathBuf>,
    profile: String,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        project: PathBuf::from("."),
        out: None,
        profile: "dev".into(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--project" => {
                args.project = PathBuf::from(it.next().ok_or("--project needs a value")?)
            }
            "--out" => args.out = Some(PathBuf::from(it.next().ok_or("--out needs a value")?)),
            "--profile" => args.profile = it.next().ok_or("--profile needs a value")?,
            "-h" | "--help" => {
                println!(
                    "usage: quoag [--project <maven module>] [--out <dir>] [--profile <quarkus profile>]\n\n\
                     Reads <project>/src/main/java, <project>/src/main/resources/application.properties and\n\
                     <project>/pom.xml and writes openapi.json and openapi.yaml to --out (default: the\n\
                     configured quarkus.smallrye-openapi.store-schema-directory)."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

/// Properties of `application.properties` for the given profile (`%profile.` keys override plain ones).
fn read_properties(path: &Path, profile: &str) -> Result<HashMap<String, String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut plain = HashMap::new();
    let mut profiled = HashMap::new();
    let mut logical = String::new();
    for raw in text.lines() {
        let line = raw.trim_start();
        if logical.is_empty() && (line.is_empty() || line.starts_with('#') || line.starts_with('!'))
        {
            continue;
        }
        if let Some(stripped) = line.strip_suffix('\\') {
            logical.push_str(stripped);
            continue;
        }
        logical.push_str(line);
        let entry = std::mem::take(&mut logical);
        let sep = entry.find(['=', ':']).unwrap_or(entry.len());
        let key = entry[..sep].trim().to_string();
        let value = entry.get(sep + 1..).unwrap_or("").trim().to_string();
        if let Some(rest) = key.strip_prefix('%') {
            if let Some((p, k)) = rest.split_once('.')
                && p.split(',').any(|x| x == profile)
            {
                profiled.insert(k.to_string(), value);
            }
        } else {
            plain.insert(key, value);
        }
    }
    plain.extend(profiled);
    Ok(plain)
}

/// `<version>` and `<artifactId>` of the project itself (not of the parent).
fn read_pom(path: &Path) -> Result<(String, String), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut depth = 0i32;
    let mut version = None;
    let mut artifact = None;
    let mut rest = text.as_str();
    while let Some(start) = rest.find('<') {
        let after = &rest[start + 1..];
        let end = after.find('>').ok_or("malformed pom.xml")?;
        let tag = &after[..end];
        rest = &after[end + 1..];
        if tag.starts_with('?') || tag.starts_with('!') || tag.ends_with('/') {
            continue;
        }
        if tag.starts_with('/') {
            depth -= 1;
            continue;
        }
        let name = tag.split_whitespace().next().unwrap_or("");
        if depth == 1 && (name == "version" || name == "artifactId") {
            let value_end = rest.find('<').unwrap_or(rest.len());
            let value = rest[..value_end].trim().to_string();
            if name == "version" && version.is_none() {
                version = Some(value);
            } else if name == "artifactId" && artifact.is_none() {
                artifact = Some(value);
            }
        }
        depth += 1;
    }
    Ok((
        artifact.ok_or("pom.xml has no artifactId")?,
        version.ok_or("pom.xml has no version")?,
    ))
}

const OPENAPI_PREFIX: &str = "quarkus.smallrye-openapi.";

/// Settings of the default document that this tool understands, with the values Quarkus uses by default.
const KNOWN_KEYS: &[&str] = &[
    "store-schema-directory",
    "security-scheme",
    "security-scheme-name",
    "security-scheme-description",
    "auto-add-tags",
    "auto-add-server",
    "servers",
    "scan-profiles",
    "scan-exclude-profiles",
    "info-title",
    "info-version",
    "operation-id-strategy",
    "jwt-security-scheme-value",
    "jwt-bearer-format",
];

struct Settings {
    cfg: scan::Config,
    store_dir: Option<String>,
    scheme: Option<quarkus::SecurityScheme>,
    scheme_name: String,
    title: String,
    version: String,
}

fn settings(
    props: &HashMap<String, String>,
    artifact: &str,
    pom_version: &str,
) -> Result<Settings, String> {
    let mut errors = Vec::new();
    let mut names: Vec<&String> = props.keys().collect();
    names.sort();
    for k in names {
        if let Some(rest) = k.strip_prefix(OPENAPI_PREFIX) {
            if KNOWN_KEYS.contains(&rest) {
                continue;
            }
            // `quarkus.smallrye-openapi.<document>.<key>` belongs to a named document, which is not stored
            if let Some((_, key)) = rest.split_once('.')
                && KNOWN_KEYS.contains(&key)
            {
                continue;
            }
            errors.push(format!("unsupported setting {k}"));
        } else if k.starts_with("mp.openapi.")
            && k != "mp.openapi.extensions.smallrye.operationIdStrategy"
        {
            errors.push(format!("unsupported setting {k}"));
        }
    }
    let get = |k: &str| props.get(&format!("{OPENAPI_PREFIX}{k}")).cloned();
    if get("auto-add-tags").as_deref() != Some("false") {
        errors.push(format!("{OPENAPI_PREFIX}auto-add-tags must be false"));
    }
    if get("auto-add-server").as_deref() != Some("false") {
        errors.push(format!("{OPENAPI_PREFIX}auto-add-server must be false"));
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    let list = |v: Option<String>| -> Vec<String> {
        v.map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default()
    };
    let strategy = get("operation-id-strategy")
        .or_else(|| {
            props
                .get("mp.openapi.extensions.smallrye.operationIdStrategy")
                .cloned()
        })
        .map(
            |s| match s.trim().to_lowercase().replace('_', "-").as_str() {
                "method" => "METHOD".to_string(),
                "class-method" => "CLASS_METHOD".to_string(),
                "package-class-method" => "PACKAGE_CLASS_METHOD".to_string(),
                other => other.to_string(),
            },
        );
    if strategy.as_deref().is_some_and(|s| s != "METHOD") {
        return Err(format!(
            "operation id strategy {strategy:?} is not supported"
        ));
    }
    let scheme_name = get("security-scheme-name").unwrap_or_else(|| "SecurityScheme".into());
    let scheme = match get("security-scheme").as_deref() {
        None => None,
        Some("jwt") => Some(quarkus::SecurityScheme {
            name: scheme_name.clone(),
            description: get("security-scheme-description")
                .unwrap_or_else(|| "Authentication".into()),
            scheme_type: "http".into(),
            scheme: get("jwt-security-scheme-value").unwrap_or_else(|| "bearer".into()),
            bearer_format: Some(get("jwt-bearer-format").unwrap_or_else(|| "JWT".into())),
        }),
        Some(other) => return Err(format!("security scheme {other} is not supported")),
    };
    let mut cfg = scan::Config {
        operation_id_strategy: strategy,
        scan_profiles: list(get("scan-profiles")),
        scan_exclude_profiles: list(get("scan-exclude-profiles")),
        servers: list(get("servers")),
        info_title: get("info-title"),
        info_version: get("info-version"),
        ..Default::default()
    };
    // MediaTypeConfigSource of quarkus-smallrye-openapi
    cfg.default_produces = Some(vec!["application/json".into()]);
    cfg.default_consumes = Some(vec!["application/json".into()]);
    cfg.default_primitives_produces = Some(vec!["text/plain".into()]);
    cfg.default_primitives_consumes = Some(vec!["text/plain".into()]);
    cfg.default_streaming_produces = Some(vec!["application/octet-stream".into()]);
    cfg.default_streaming_consumes = Some(vec!["application/octet-stream".into()]);
    let name = props
        .get("quarkus.application.name")
        .cloned()
        .unwrap_or_else(|| artifact.to_string());
    let version = props
        .get("quarkus.application.version")
        .cloned()
        .unwrap_or_else(|| pom_version.to_string());
    Ok(Settings {
        cfg,
        store_dir: get("store-schema-directory"),
        scheme,
        scheme_name,
        title: format!("{name} API"),
        version,
    })
}

fn sort_map_prop(m: &model::M, key: &str) {
    if let Some(mut map) = model::get_map(m, key) {
        map.sort_keys();
        model::set_v(m, key, V::Map(map));
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let project = &args.project;
    let props = read_properties(
        &project.join("src/main/resources/application.properties"),
        &args.profile,
    )?;
    let (artifact, pom_version) = read_pom(&project.join("pom.xml"))?;
    let s = settings(&props, &artifact, &pom_version)?;
    let out_dir = match (&args.out, &s.store_dir) {
        (Some(o), _) => o.clone(),
        (None, Some(d)) => project.join(d),
        (None, None) => {
            return Err(
                "no --out given and quarkus.smallrye-openapi.store-schema-directory is not set"
                    .into(),
            );
        }
    };

    let mut idx = index::Index::load(&[project.join("src/main/java")], &[])?;
    let extra = indexed_jdk_types(&idx);
    idx.add_extra(&extra);
    let mut ctx = scan::Ctx::new(&idx, &s.cfg);
    let openapi = ctx.openapi.clone();
    model::set_v(&openapi, "openapi", V::Str("3.1.0".into()));

    // OpenApiAnnotationScanner.processClassSchemas: classes annotated with @Schema, in name order
    let mut schema_classes: Vec<&index::ClassInfo> = idx
        .classes
        .iter()
        .filter(|c| c.annotation(scan::util::SCHEMA).is_some())
        .collect();
    schema_classes.sort_by(|a, b| a.name.cmp(&b.name));
    for c in schema_classes {
        scan::schema::type_to_schema(&mut ctx, &index::Type::class(&c.name), None);
    }

    let mut refs = Vec::new();
    scan::jaxrs::scan(&mut ctx, &mut refs);
    let errors = ctx.take_errors();
    if !errors.is_empty() {
        return Err(format!("not supported:\n  {}", errors.join("\n  ")));
    }

    // sortTags + sortMaps
    if let Some(mut tags) = model::get_obj_list(&openapi, "tags") {
        tags.sort_by_key(|t| model::get_str(t, "name").unwrap_or_default());
        model::set_v(
            &openapi,
            "tags",
            V::List(tags.into_iter().map(V::Obj).collect()),
        );
    }
    if let Some(paths) = model::get_obj(&openapi, "paths") {
        let mut p = paths.borrow_mut();
        p.props.sort_keys();
    }
    if let Some(components) = model::get_obj(&openapi, "components") {
        for key in [
            "callbacks",
            "examples",
            "headers",
            "links",
            "parameters",
            "requestBodies",
            "responses",
            "schemas",
            "securitySchemes",
        ] {
            sort_map_prop(&components, key);
        }
    }

    // Quarkus filters (sorted by class name: OperationFilter, SecurityConfigFilter), then DefaultInfoFilter
    quarkus::operation_filter(&idx, &openapi, &refs, &s.scheme_name);
    if let Some(scheme) = &s.scheme {
        quarkus::security_config_filter(&openapi, scheme);
    }
    let title = s.cfg.info_title.clone().unwrap_or(s.title.clone());
    let version = s.cfg.info_version.clone().unwrap_or(s.version.clone());
    quarkus::default_info_filter(&openapi, &title, &version);
    // ConfigUtil.applyConfig: servers
    if !s.cfg.servers.is_empty() {
        let servers = s
            .cfg
            .servers
            .iter()
            .map(|url| {
                let server = model::new(MK::Server);
                model::set_v(&server, "url", V::Str(url.clone()));
                V::Obj(server)
            })
            .collect();
        model::set_v(&openapi, "servers", V::List(servers));
    }

    let node = writer::to_node(&openapi);
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    std::fs::write(out_dir.join("openapi.json"), writer::to_json(&node))
        .map_err(|e| e.to_string())?;
    std::fs::write(out_dir.join("openapi.yaml"), writer::to_yaml(&node))
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

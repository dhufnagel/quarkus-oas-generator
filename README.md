# quoag

Generates `openapi.json` and `openapi.yaml` of a Quarkus project from its Java sources, byte-identical to what
`quarkus-smallrye-openapi` stores in its `store-schema-directory` during `quarkus:build`.
It neither compiles nor runs Java and takes about 0.2 s instead of the Maven/Quarkus build.

```bash
cargo build --release
target/release/quoag --project path/to/quarkus-app   # writes to the store-schema-directory
target/release/quoag --project path/to/quarkus-app --out /tmp/spec
```

`--profile` selects the Quarkus profile for `application.properties` (default `dev`, as in the pre-commit hook).

## How it works

- `java`: parses the sources with [jtk-parser](https://github.com/dhufnagel/jtk) and reduces the syntax tree to the
  declarations the index needs, with literal values and modifier keywords read from the source.
- `index`: a Jandex-like class index — name resolution, constant evaluation in annotations, records with
  annotation propagation by `@Target`.
- `scan`: a port of the smallrye-open-api 4.3 scanner parts Quarkus uses (JAX-RS resources, parameters, request
  bodies, responses, schemas, bean validation, schema registry), keeping its processing order, because the order
  decides component names and whether a type is written as `$ref`.
- `quarkus`: the additions of quarkus-smallrye-openapi (operation summaries, security requirements with 401/403,
  400 responses, security scheme, default info).
- `writer`: Jackson's JSON pretty printer and jackson-dataformat-yaml/SnakeYAML output, including line folding.

## Limits

- SmallRye only writes JDK types such as `UUID` or `LocalTime` as `$ref` when they are in Quarkus' Jandex index.
  Quarkus indexes them as a side effect of processing beans, resources and entities; the generator approximates
  this with the types used in fields and method signatures of such classes (`indexed_jdk_types` in `main.rs`).
- Constructs the port does not cover (e.g. `@JsonValue` on enums, `@JsonUnwrapped`, sub-resource locators,
  `@Parameter`) and unknown `quarkus.smallrye-openapi.*` settings make it fail instead of writing a different
  document.

The Quarkus build stays the reference: if the outputs differ, `validate-openapi` in CI fails.

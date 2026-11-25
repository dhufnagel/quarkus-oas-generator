//! Facts about classes outside the scanned sources: annotation targets, constants and JDK supertypes.

/// Element kinds an annotation may be applied to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ElementKind {
    Type,
    Field,
    Method,
    Parameter,
    Constructor,
    RecordComponent,
    TypeUse,
    AnnotationType,
}

use ElementKind::*;

const BEAN_VALIDATION: &[ElementKind] = &[
    Method,
    Field,
    AnnotationType,
    Constructor,
    Parameter,
    TypeUse,
];

/// `@Target` of well-known annotations. `None` means "all declaration contexts" (no `@Target`).
pub fn annotation_targets(name: &str) -> Option<&'static [ElementKind]> {
    if name.starts_with("jakarta.validation.constraints.")
        || name.starts_with("javax.validation.constraints.")
    {
        return Some(BEAN_VALIDATION);
    }
    Some(match name {
        "jakarta.validation.Valid" => &[Method, Field, Constructor, Parameter, TypeUse],
        "com.fasterxml.jackson.annotation.JsonProperty" => {
            &[AnnotationType, Field, Method, Parameter]
        }
        "com.fasterxml.jackson.annotation.JsonValue" => &[AnnotationType, Method, Field],
        "com.fasterxml.jackson.annotation.JsonFormat" => {
            &[AnnotationType, Field, Method, Parameter, Type]
        }
        "com.fasterxml.jackson.annotation.JsonIgnore" => {
            &[AnnotationType, Method, Constructor, Field]
        }
        "com.fasterxml.jackson.annotation.JsonView" => {
            &[AnnotationType, Method, Field, Parameter, Type]
        }
        "jakarta.json.bind.annotation.JsonbProperty" => &[AnnotationType, Method, Field, Parameter],
        "jakarta.json.bind.annotation.JsonbTransient" => &[AnnotationType, Field, Method],
        "jakarta.json.bind.annotation.JsonbTypeAdapter" => {
            &[AnnotationType, Type, Field, Method, Parameter]
        }
        "jakarta.json.bind.annotation.JsonbDateFormat" => {
            &[AnnotationType, Field, Method, Type, Parameter]
        }
        "jakarta.json.bind.annotation.JsonbNumberFormat" => {
            &[AnnotationType, Field, Method, Type, Parameter]
        }
        "jakarta.json.bind.annotation.JsonbTypeInfo" => &[AnnotationType, Type],
        "org.eclipse.microprofile.openapi.annotations.media.Schema" => {
            &[Field, Method, Parameter, Type]
        }
        "org.eclipse.microprofile.openapi.annotations.media.ArraySchema" => {
            &[Field, Method, Parameter, Type]
        }
        "org.eclipse.microprofile.openapi.annotations.parameters.Parameter" => {
            &[Parameter, Method, Field]
        }
        "org.eclipse.microprofile.openapi.annotations.parameters.RequestBody" => {
            &[Parameter, Method]
        }
        "org.jboss.resteasy.reactive.RestForm"
        | "org.jboss.resteasy.reactive.RestQuery"
        | "org.jboss.resteasy.reactive.RestPath"
        | "org.jboss.resteasy.reactive.RestHeader"
        | "org.jboss.resteasy.reactive.RestCookie"
        | "org.jboss.resteasy.reactive.RestMatrix"
        | "org.jboss.resteasy.reactive.PartType" => &[Field, Parameter],
        "jakarta.ws.rs.PathParam"
        | "jakarta.ws.rs.QueryParam"
        | "jakarta.ws.rs.BeanParam"
        | "jakarta.ws.rs.DefaultValue"
        | "jakarta.ws.rs.FormParam"
        | "jakarta.ws.rs.HeaderParam"
        | "jakarta.ws.rs.CookieParam"
        | "jakarta.ws.rs.MatrixParam" => &[Parameter, Method, Field],
        "jakarta.ws.rs.core.Context" => &[Parameter, Method, Field],
        "java.lang.Deprecated" => &[Constructor, Field, Method, Parameter, Type],
        "java.lang.Override" => &[Method],
        "java.lang.SuppressWarnings" => &[Type, Field, Method, Parameter, Constructor],
        "jakarta.persistence.Column"
        | "jakarta.persistence.JoinColumn"
        | "jakarta.persistence.Id" => &[Method, Field],
        "jakarta.persistence.Convert" => &[Method, Field, Type],
        _ => return None,
    })
}

/// String/number constants of library classes that appear in annotations.
pub fn external_constant(class: &str, field: &str) -> Option<&'static str> {
    let value = match (class, field) {
        ("jakarta.ws.rs.core.MediaType", f) => match f {
            "WILDCARD" => "*/*",
            "APPLICATION_XML" => "application/xml",
            "APPLICATION_ATOM_XML" => "application/atom+xml",
            "APPLICATION_XHTML_XML" => "application/xhtml+xml",
            "APPLICATION_SVG_XML" => "application/svg+xml",
            "APPLICATION_JSON" => "application/json",
            "APPLICATION_FORM_URLENCODED" => "application/x-www-form-urlencoded",
            "MULTIPART_FORM_DATA" => "multipart/form-data",
            "APPLICATION_OCTET_STREAM" => "application/octet-stream",
            "TEXT_PLAIN" => "text/plain",
            "TEXT_XML" => "text/xml",
            "TEXT_HTML" => "text/html",
            "SERVER_SENT_EVENTS" => "text/event-stream",
            "APPLICATION_JSON_PATCH_JSON" => "application/json-patch+json",
            _ => return None,
        },
        ("java.nio.charset.StandardCharsets", _) => return None,
        _ => return None,
    };
    Some(value)
}

/// Direct supertypes (superclass first, then interfaces) of JDK types relevant for `isA` checks.
pub fn jdk_supertypes(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "java.lang.Object" => &[],
        "java.lang.Iterable" => &["java.lang.Object"],
        "java.util.Collection" => &["java.lang.Object", "java.lang.Iterable"],
        "java.util.List" | "java.util.Set" | "java.util.Queue" => {
            &["java.lang.Object", "java.util.Collection"]
        }
        "java.util.SequencedCollection" => &["java.lang.Object", "java.util.Collection"],
        "java.util.SortedSet" | "java.util.NavigableSet" => &["java.lang.Object", "java.util.Set"],
        "java.util.Deque" => &["java.lang.Object", "java.util.Queue"],
        "java.util.ArrayList" | "java.util.LinkedList" => {
            &["java.util.AbstractList", "java.util.List"]
        }
        "java.util.AbstractList" => &["java.util.AbstractCollection", "java.util.List"],
        "java.util.AbstractCollection" => &["java.lang.Object", "java.util.Collection"],
        "java.util.HashSet"
        | "java.util.LinkedHashSet"
        | "java.util.TreeSet"
        | "java.util.EnumSet" => &["java.util.AbstractSet", "java.util.Set"],
        "java.util.AbstractSet" => &["java.util.AbstractCollection", "java.util.Set"],
        "java.util.Map" => &["java.lang.Object"],
        "java.util.SortedMap" | "java.util.NavigableMap" => &["java.lang.Object", "java.util.Map"],
        "java.util.HashMap"
        | "java.util.LinkedHashMap"
        | "java.util.TreeMap"
        | "java.util.EnumMap" => &["java.util.AbstractMap", "java.util.Map"],
        "java.util.AbstractMap" => &["java.lang.Object", "java.util.Map"],
        "java.util.stream.BaseStream" => &["java.lang.Object", "java.lang.AutoCloseable"],
        "java.util.stream.Stream" => &["java.lang.Object", "java.util.stream.BaseStream"],
        "java.lang.Enum" => &[
            "java.lang.Object",
            "java.lang.Comparable",
            "java.io.Serializable",
        ],
        "java.lang.String" => &[
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.CharSequence",
        ],
        "java.util.Optional" => &["java.lang.Object"],
        "java.util.UUID" => &[
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.Comparable",
        ],
        "java.util.concurrent.CompletionStage" => &["java.lang.Object"],
        "java.util.concurrent.CompletableFuture" => &[
            "java.lang.Object",
            "java.util.concurrent.Future",
            "java.util.concurrent.CompletionStage",
        ],
        _ => return None,
    })
}

/// Classes that SmallRye's own JDK index knows (`TypeUtil.knownJavaType`).
pub fn known_java_type(name: &str) -> bool {
    matches!(
        name,
        "java.lang.Enum"
            | "java.lang.Object"
            | "java.lang.AutoCloseable"
            | "java.lang.Cloneable"
            | "java.lang.Comparable"
            | "java.io.Serializable"
            | "java.util.RandomAccess"
            | "java.lang.Boolean"
            | "java.lang.Byte"
            | "java.lang.Character"
            | "java.lang.CharSequence"
            | "java.lang.Double"
            | "java.lang.Float"
            | "java.lang.Integer"
            | "java.lang.Long"
            | "java.lang.Number"
            | "java.lang.Short"
            | "java.lang.String"
            | "java.lang.Void"
            | "java.util.Date"
            | "java.util.UUID"
            | "java.time.Duration"
            | "java.time.Instant"
            | "java.time.LocalDate"
            | "java.time.LocalDateTime"
            | "java.time.LocalTime"
            | "java.time.OffsetDateTime"
            | "java.time.OffsetTime"
            | "java.time.Period"
            | "java.time.ZonedDateTime"
            | "java.time.chrono.ChronoLocalDate"
            | "java.time.chrono.ChronoLocalDateTime"
            | "java.time.chrono.ChronoPeriod"
            | "java.time.chrono.ChronoZonedDateTime"
            | "java.time.temporal.Temporal"
            | "java.time.temporal.TemporalAccessor"
            | "java.time.temporal.TemporalAdjuster"
            | "java.time.temporal.TemporalAmount"
            | "java.lang.Iterable"
            | "java.util.Collection"
            | "java.util.Deque"
            | "java.util.List"
            | "java.util.Map"
            | "java.util.NavigableMap"
            | "java.util.NavigableSet"
            | "java.util.Queue"
            | "java.util.Set"
            | "java.util.SortedMap"
            | "java.util.SortedSet"
            | "java.util.concurrent.BlockingDeque"
            | "java.util.concurrent.BlockingQueue"
            | "java.util.concurrent.ConcurrentMap"
            | "java.util.concurrent.ConcurrentNavigableMap"
            | "java.util.concurrent.TransferQueue"
            | "java.util.AbstractCollection"
            | "java.util.AbstractList"
            | "java.util.AbstractMap"
            | "java.util.AbstractQueue"
            | "java.util.AbstractSequentialList"
            | "java.util.AbstractSet"
            | "java.util.EnumSet"
            | "java.util.ArrayDeque"
            | "java.util.ArrayList"
            | "java.util.Dictionary"
            | "java.util.EnumMap"
            | "java.util.HashMap"
            | "java.util.HashSet"
            | "java.util.Hashtable"
            | "java.util.IdentityHashMap"
            | "java.util.LinkedHashMap"
            | "java.util.LinkedHashSet"
            | "java.util.LinkedList"
            | "java.util.PriorityQueue"
            | "java.util.Properties"
            | "java.util.Stack"
            | "java.util.TreeMap"
            | "java.util.TreeSet"
            | "java.util.Vector"
            | "java.util.concurrent.ArrayBlockingQueue"
            | "java.util.concurrent.ConcurrentHashMap"
            | "java.util.concurrent.ConcurrentLinkedDeque"
            | "java.util.concurrent.ConcurrentLinkedQueue"
            | "java.util.concurrent.ConcurrentSkipListMap"
            | "java.util.concurrent.ConcurrentSkipListSet"
            | "java.util.concurrent.CopyOnWriteArrayList"
            | "java.util.concurrent.CopyOnWriteArraySet"
            | "java.util.concurrent.DelayQueue"
            | "java.util.concurrent.LinkedBlockingDeque"
            | "java.util.concurrent.LinkedBlockingQueue"
            | "java.util.concurrent.LinkedTransferQueue"
            | "java.util.concurrent.PriorityBlockingQueue"
            | "java.util.concurrent.SynchronousQueue"
            | "java.util.stream.BaseStream"
            | "java.util.stream.Stream"
            | "java.util.stream.IntStream"
            | "java.util.stream.LongStream"
            | "java.util.stream.DoubleStream"
            | "java.util.concurrent.CompletionStage"
            | "java.util.concurrent.CompletableFuture"
            | "java.util.concurrent.Future"
            | "java.lang.constant.Constable"
            | "java.lang.constant.ConstantDesc"
    )
}

/// Members of library packages that may be imported on demand (`import pkg.*;`).
pub fn package_members(pkg: &str) -> Option<&'static [&'static str]> {
    let p = pkg.replace("javax.", "jakarta.");
    Some(match p.as_str() {
        "jakarta.ws.rs" => &[
            "ApplicationPath",
            "BeanParam",
            "ConstrainedTo",
            "Consumes",
            "CookieParam",
            "DefaultValue",
            "DELETE",
            "Encoded",
            "FormParam",
            "GET",
            "HEAD",
            "HeaderParam",
            "HttpMethod",
            "MatrixParam",
            "NameBinding",
            "OPTIONS",
            "PATCH",
            "Path",
            "PathParam",
            "POST",
            "Produces",
            "PUT",
            "QueryParam",
            "SeBootstrap",
            "BadRequestException",
            "ClientErrorException",
            "ForbiddenException",
            "InternalServerErrorException",
            "NotAcceptableException",
            "NotAllowedException",
            "NotAuthorizedException",
            "NotFoundException",
            "NotSupportedException",
            "ProcessingException",
            "RedirectionException",
            "ServerErrorException",
            "ServiceUnavailableException",
            "WebApplicationException",
            "Priorities",
            "RuntimeType",
        ],
        "jakarta.ws.rs.core" => &[
            "Response",
            "MediaType",
            "Context",
            "UriInfo",
            "HttpHeaders",
            "Request",
            "SecurityContext",
            "Application",
            "StreamingOutput",
            "MultivaluedMap",
            "MultivaluedHashMap",
            "Cookie",
            "NewCookie",
            "CacheControl",
            "EntityTag",
            "Form",
            "GenericEntity",
            "GenericType",
            "Link",
            "PathSegment",
            "UriBuilder",
            "Variant",
            "Configuration",
            "Feature",
            "FeatureContext",
            "NoContentException",
            "EntityPart",
            "Configurable",
        ],
        "java.util" => &[
            "List",
            "Map",
            "Set",
            "Optional",
            "OptionalInt",
            "OptionalLong",
            "OptionalDouble",
            "UUID",
            "Collection",
            "ArrayList",
            "LinkedList",
            "HashMap",
            "HashSet",
            "LinkedHashMap",
            "LinkedHashSet",
            "TreeMap",
            "TreeSet",
            "SortedMap",
            "SortedSet",
            "NavigableMap",
            "NavigableSet",
            "Collections",
            "Arrays",
            "Objects",
            "Comparator",
            "Iterator",
            "Locale",
            "Date",
            "Currency",
            "Deque",
            "ArrayDeque",
            "Queue",
            "EnumSet",
            "EnumMap",
            "Random",
            "Scanner",
            "StringJoiner",
            "Base64",
            "Calendar",
            "TimeZone",
            "Properties",
            "AbstractMap",
            "AbstractList",
            "BitSet",
            "IdentityHashMap",
            "WeakHashMap",
            "Stack",
            "Vector",
            "NoSuchElementException",
            "Spliterator",
            "Spliterators",
            "StringTokenizer",
            "Formatter",
            "ResourceBundle",
            "ServiceLoader",
            "PriorityQueue",
            "SequencedCollection",
            "SequencedMap",
            "SequencedSet",
            "HexFormat",
            "Iterable",
        ],
        "java.time" => &[
            "LocalDate",
            "LocalDateTime",
            "LocalTime",
            "OffsetDateTime",
            "OffsetTime",
            "ZonedDateTime",
            "Instant",
            "Duration",
            "Period",
            "ZoneId",
            "ZoneOffset",
            "DayOfWeek",
            "Month",
            "Year",
            "YearMonth",
            "Clock",
            "MonthDay",
            "DateTimeException",
        ],
        "jakarta.validation.constraints" => &[
            "NotNull",
            "NotBlank",
            "NotEmpty",
            "Size",
            "Pattern",
            "Min",
            "Max",
            "Positive",
            "PositiveOrZero",
            "Negative",
            "NegativeOrZero",
            "Email",
            "Past",
            "PastOrPresent",
            "Future",
            "FutureOrPresent",
            "DecimalMin",
            "DecimalMax",
            "Digits",
            "AssertTrue",
            "AssertFalse",
            "Null",
        ],
        "java.io" => &[
            "File",
            "InputStream",
            "OutputStream",
            "IOException",
            "Serializable",
            "UncheckedIOException",
            "Reader",
            "Writer",
            "ByteArrayInputStream",
            "ByteArrayOutputStream",
            "BufferedReader",
            "BufferedWriter",
            "InputStreamReader",
            "OutputStreamWriter",
            "PrintWriter",
            "PrintStream",
            "StringReader",
            "StringWriter",
            "FileInputStream",
            "FileOutputStream",
            "Closeable",
            "EOFException",
            "FileNotFoundException",
        ],
        "org.eclipse.microprofile.openapi.annotations.tags" => &["Tag", "Tags"],
        "org.eclipse.microprofile.openapi.annotations.responses" => {
            &["APIResponse", "APIResponses", "APIResponseSchema"]
        }
        "org.eclipse.microprofile.openapi.annotations.media" => &[
            "Schema",
            "Content",
            "ExampleObject",
            "DiscriminatorMapping",
            "SchemaProperty",
            "Encoding",
            "DependentRequired",
            "DependentSchema",
            "PatternProperty",
        ],
        "org.eclipse.microprofile.openapi.annotations.enums" => &[
            "SchemaType",
            "ParameterIn",
            "ParameterStyle",
            "Explode",
            "SecuritySchemeType",
            "SecuritySchemeIn",
        ],
        "org.eclipse.microprofile.openapi.annotations.parameters" => &[
            "Parameter",
            "Parameters",
            "RequestBody",
            "RequestBodySchema",
        ],
        "org.eclipse.microprofile.openapi.annotations" => {
            &["Operation", "OpenAPIDefinition", "Components"]
        }
        "org.jboss.resteasy.reactive" => &[
            "RestPath",
            "RestQuery",
            "RestForm",
            "RestHeader",
            "RestCookie",
            "RestMatrix",
            "RestResponse",
            "PartType",
            "ResponseHeader",
            "ResponseStatus",
            "NoCache",
            "Separator",
            "MultipartForm",
            "Cache",
        ],
        "jakarta.inject" => &[
            "Inject",
            "Named",
            "Singleton",
            "Provider",
            "Qualifier",
            "Scope",
        ],
        _ => return None,
    })
}

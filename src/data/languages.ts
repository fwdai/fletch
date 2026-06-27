// Single source of truth mapping a file extension to its highlight.js
// language id (for syntax highlighting) and a human label (for the editor's
// meta bar). Anything unmapped falls back to auto-detection + the uppercased
// extension.
export interface LangInfo {
  /** highlight.js language id; omit to fall back to auto-detection. */
  hljs?: string;
  label: string;
}

const LANGUAGES: Record<string, LangInfo> = {
  ts: { hljs: "typescript", label: "TypeScript" },
  tsx: { hljs: "typescript", label: "TypeScript JSX" },
  mts: { hljs: "typescript", label: "TypeScript" },
  cts: { hljs: "typescript", label: "TypeScript" },
  js: { hljs: "javascript", label: "JavaScript" },
  jsx: { hljs: "javascript", label: "JavaScript JSX" },
  mjs: { hljs: "javascript", label: "JavaScript" },
  cjs: { hljs: "javascript", label: "JavaScript" },
  json: { hljs: "json", label: "JSON" },
  jsonc: { hljs: "json", label: "JSON" },
  py: { hljs: "python", label: "Python" },
  rb: { hljs: "ruby", label: "Ruby" },
  rs: { hljs: "rust", label: "Rust" },
  go: { hljs: "go", label: "Go" },
  java: { hljs: "java", label: "Java" },
  kt: { hljs: "kotlin", label: "Kotlin" },
  kts: { hljs: "kotlin", label: "Kotlin" },
  swift: { hljs: "swift", label: "Swift" },
  scala: { hljs: "scala", label: "Scala" },
  c: { hljs: "c", label: "C" },
  h: { hljs: "c", label: "C" },
  cpp: { hljs: "cpp", label: "C++" },
  cc: { hljs: "cpp", label: "C++" },
  cxx: { hljs: "cpp", label: "C++" },
  hpp: { hljs: "cpp", label: "C++" },
  hh: { hljs: "cpp", label: "C++" },
  cs: { hljs: "csharp", label: "C#" },
  php: { hljs: "php", label: "PHP" },
  css: { hljs: "css", label: "CSS" },
  scss: { hljs: "scss", label: "SCSS" },
  sass: { hljs: "scss", label: "Sass" },
  less: { hljs: "less", label: "Less" },
  html: { hljs: "xml", label: "HTML" },
  htm: { hljs: "xml", label: "HTML" },
  xml: { hljs: "xml", label: "XML" },
  svg: { hljs: "xml", label: "SVG" },
  vue: { hljs: "xml", label: "Vue" },
  md: { hljs: "markdown", label: "Markdown" },
  markdown: { hljs: "markdown", label: "Markdown" },
  yml: { hljs: "yaml", label: "YAML" },
  yaml: { hljs: "yaml", label: "YAML" },
  toml: { hljs: "ini", label: "TOML" },
  ini: { hljs: "ini", label: "INI" },
  cfg: { hljs: "ini", label: "INI" },
  sh: { hljs: "bash", label: "Shell" },
  bash: { hljs: "bash", label: "Shell" },
  zsh: { hljs: "bash", label: "Shell" },
  sql: { hljs: "sql", label: "SQL" },
  graphql: { hljs: "graphql", label: "GraphQL" },
  gql: { hljs: "graphql", label: "GraphQL" },
  lua: { hljs: "lua", label: "Lua" },
  r: { hljs: "r", label: "R" },
  pl: { hljs: "perl", label: "Perl" },
  pm: { hljs: "perl", label: "Perl" },
  dart: { hljs: "dart", label: "Dart" },
  dockerfile: { hljs: "dockerfile", label: "Dockerfile" },
  makefile: { hljs: "makefile", label: "Makefile" },
};

/** highlight.js language id for a file extension, or undefined to auto-detect. */
export function hljsLang(ext: string): string | undefined {
  return LANGUAGES[(ext || "").toLowerCase()]?.hljs;
}

/** Human-readable language label for the editor meta bar. */
export function langLabel(ext: string): string {
  if (!ext) return "Plain Text";
  return LANGUAGES[ext.toLowerCase()]?.label ?? ext.toUpperCase();
}

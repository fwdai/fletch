import {
  Archive,
  ArchiveRestore,
  ArrowDown,
  ArrowRight,
  ArrowUp,
  Bot,
  Box,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronsDownUp,
  ChevronUp,
  Clock,
  Code,
  Combine,
  Copy,
  Dot,
  Download,
  Ellipsis,
  ExternalLink,
  File,
  FileDiff,
  FlaskConical,
  Folder,
  GitBranch,
  GitCommitHorizontal,
  GitMerge,
  GitPullRequest,
  GripVertical,
  Hand,
  History,
  Inbox,
  Layers,
  LayoutGrid,
  Map as MapIcon,
  Minus,
  Moon,
  MoreHorizontal,
  NotebookPen,
  PanelLeft,
  PanelRight,
  Paperclip,
  Pause,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Repeat,
  Search,
  Settings,
  Sparkles,
  Square,
  Sun,
  Terminal,
  Trash2,
  Upload,
  User,
  Wrench,
  X,
  Zap,
} from "lucide-react";
import type { ComponentType, CSSProperties, ReactNode, SVGProps } from "react";
import { createElement } from "react";
import { LANDMARK_GLYPHS } from "@/data/landmarks";

// Lucide v1 removed brand icons, so we keep the GitHub mark inline.
type IconComponentProps = Omit<SVGProps<SVGSVGElement>, "ref"> & {
  size?: number | string;
  strokeWidth?: number | string;
};

function Github({ size = 24, strokeWidth, className, style, ...rest }: IconComponentProps) {
  void strokeWidth;
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width={size}
      height={size}
      viewBox="0 0 16 16"
      className={className}
      style={style}
      {...rest}
    >
      <path d="M8 1.5a6.5 6.5 0 0 0-2.1 12.7c.3 0 .4-.1.4-.3v-1.2c-1.8.4-2.2-.8-2.2-.8-.3-.7-.7-.9-.7-.9-.6-.4 0-.4 0-.4.6 0 1 .6 1 .6.6 1 1.5.7 1.9.6.1-.4.2-.7.4-.9-1.4-.2-2.9-.7-2.9-3.2 0-.7.3-1.3.7-1.7-.1-.2-.3-.9.1-1.8 0 0 .6-.2 1.8.6.5-.1 1.1-.2 1.6-.2.6 0 1.1.1 1.6.2 1.2-.8 1.8-.6 1.8-.6.3.9.1 1.6.1 1.8.4.4.7 1 .7 1.7 0 2.5-1.5 3-2.9 3.2.2.2.4.6.4 1.2v1.8c0 .2.1.4.4.3A6.5 6.5 0 0 0 8 1.5z" />
    </svg>
  );
}

const ICON_COMPONENTS = {
  dot: Dot,
  chevR: ChevronRight,
  chevD: ChevronDown,
  chevU: ChevronUp,
  chevL: ChevronLeft,
  arrowR: ArrowRight,
  close: X,
  plus: Plus,
  minus: Minus,
  check: Check,
  more: MoreHorizontal,
  search: Search,
  refresh: RefreshCw,
  settings: Settings,
  user: User,
  bot: Bot,
  cube: Box,
  layers: Layers,
  map: MapIcon,
  panelGrid: LayoutGrid,
  folder: Folder,
  file: File,
  copy: Copy,
  download: Download,
  diff: FileDiff,
  shrink: ChevronsDownUp,
  code: Code,
  combine: Combine,
  terminal: Terminal,
  branch: GitBranch,
  commit: GitCommitHorizontal,
  merge: GitMerge,
  pr: GitPullRequest,
  push: ArrowUp,
  arrowDown: ArrowDown,
  loop: Repeat,
  github: Github,
  play: Play,
  pause: Pause,
  stop: Square,
  attach: Paperclip,
  upload: Upload,
  external: ExternalLink,
  arrowUp: ArrowUp,
  sidebarL: PanelLeft,
  sidebarR: PanelRight,
  sparkle: Sparkles,
  inbox: Inbox,
  edit: Pencil,
  notebookPen: NotebookPen,
  trash: Trash2,
  thinking: Ellipsis,
  wrench: Wrench,
  flask: FlaskConical,
  history: History,
  archive: Archive,
  archiveRestore: ArchiveRestore,
  moon: Moon,
  sun: Sun,
  zap: Zap,
  clock: Clock,
  hand: Hand,
  grip: GripVertical,
} satisfies Record<string, ComponentType<IconComponentProps>>;

// Originally rendered with fill="currentColor". Setting fill on the root SVG
// is enough — inner shapes inherit it.
const FILLED: ReadonlySet<IconName> = new Set(["play", "pause", "stop", "github", "dot"]);

export type IconName = keyof typeof ICON_COMPONENTS;

interface IconProps {
  name: IconName;
  size?: number;
  className?: string;
  strokeWidth?: number;
  style?: CSSProperties;
}

export function Icon({ name, size = 14, className, strokeWidth = 1.5, style }: IconProps) {
  const Component = ICON_COMPONENTS[name] ?? Dot;
  const filled = FILLED.has(name);
  return (
    <Component
      size={size}
      strokeWidth={strokeWidth}
      className={className}
      style={style}
      fill={filled ? "currentColor" : "none"}
    />
  );
}

interface LandmarkGlyphProps {
  name: string;
  size?: number;
  strokeWidth?: number;
  className?: string;
  style?: CSSProperties;
}

const FALLBACK_GLYPH: ReactNode = createElement("path", { d: "M2 13 L 6 7 L 9 10 L 14 5" });

export function LandmarkGlyph({
  name,
  size = 16,
  strokeWidth = 1.4,
  className,
  style,
}: LandmarkGlyphProps) {
  const glyph = LANDMARK_GLYPHS[name] ?? FALLBACK_GLYPH;
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      style={style}
    >
      {glyph}
    </svg>
  );
}

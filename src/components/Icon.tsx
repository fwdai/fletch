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
  CircleDot,
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
  Split,
  Square,
  Sun,
  Terminal,
  Trash2,
  Upload,
  User,
  Waypoints,
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
      viewBox="0 0 24 24"
      className={className}
      style={style}
      {...rest}
    >
      <path d="M12 1A10.89 10.89 0 0 0 1 11.77 10.79 10.79 0 0 0 8.52 22c.55.1.75-.23.75-.52v-1.83c-3.06.65-3.71-1.44-3.71-1.44a2.86 2.86 0 0 0-1.22-1.58c-1-.66.08-.65.08-.65a2.31 2.31 0 0 1 1.68 1.11 2.37 2.37 0 0 0 3.2.89 2.33 2.33 0 0 1 .7-1.44c-2.44-.27-5-1.19-5-5.32a4.15 4.15 0 0 1 1.11-2.91 3.78 3.78 0 0 1 .11-2.84s.93-.29 3 1.1a10.68 10.68 0 0 1 5.5 0c2.1-1.39 3-1.1 3-1.1a3.78 3.78 0 0 1 .11 2.84A4.15 4.15 0 0 1 19 11.2c0 4.14-2.58 5.05-5 5.32a2.5 2.5 0 0 1 .75 2v2.95c0 .35.2.63.75.52A10.8 10.8 0 0 0 23 11.77 10.89 10.89 0 0 0 12 1" />
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
  graph: Waypoints,
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
  split: Split,
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
  issue: CircleDot,
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

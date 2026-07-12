interface CompactPathOptions {
  homeDir?: string;
  maxLength?: number;
}

export function compactSourcePath(path: string, options: CompactPathOptions = {}) {
  const title = path;
  const home = options.homeDir?.replace(/\/+$/, "") ?? inferHomeDir(path);
  let display = path;
  if (home && (path === home || path.startsWith(`${home}/`))) {
    display = path === home ? "~" : `~/${path.slice(home.length + 1)}`;
  }

  const maxLength = options.maxLength ?? 42;
  const compacted = display.length > maxLength ? middleEllipsisPath(display) : display;
  return {
    label: `file · ${compacted}`,
    title,
  };
}

function middleEllipsisPath(path: string) {
  const parts = path.split("/").filter(Boolean);
  if (parts.length <= 2) return path;
  if (path.startsWith("~/")) {
    return `~/.../${parts.slice(-2).join("/")}`;
  }
  if (path.startsWith("/")) {
    return `/${parts[0]}/.../${parts[parts.length - 1]}`;
  }
  return `${parts[0]}/.../${parts[parts.length - 1]}`;
}

function inferHomeDir(path: string) {
  const match = path.match(/^\/(?:Users|home)\/[^/]+/);
  return match?.[0];
}

export function rememberRecentFile(files: string[], path: string, limit = 10) {
  const clean = path.trim();
  if (!clean) return files.slice(0, limit);
  return [clean, ...files.filter((file) => file !== clean)].slice(0, limit);
}

export function fileNameFromPath(path: string) {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length ? parts[parts.length - 1] : path;
}

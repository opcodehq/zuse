/** Resolve bundled brand tiles first, then fetch missing favicons on demand. */
export function createLogoResolver(iconTable: string, images: Record<string, string>) {
  const byFile = new Map(Object.entries(images).map(([path, url]) => [path.slice(path.lastIndexOf("/") + 1), url]));
  const logos = new Map<string, string>();
  for (const match of iconTable.matchAll(/"plugin-icons\/([^"]+)"\s*=>\s*(?:\{\s*)?Some\(include_bytes!\("\.\.\/assets\/plugins\/([^"]+)"\)\)/g)) {
    const url = byFile.get(match[2]!);
    if (url) logos.set(match[1]!, url);
  }
  // Remote logos are requested only by mounted, lazy-loaded <img> elements.
  // Never send custom server URLs or credentials to the favicon service.
  return function logoUrl(domain: string) {
    const host = domain.trim().toLowerCase();
    if (!/^(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,63}$/.test(host)) return null;
    return logos.get(host) ?? `https://www.google.com/s2/favicons?domain=${encodeURIComponent(host)}&sz=128`;
  };
}

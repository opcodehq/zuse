import { source } from "@/lib/source";

const documentationRoutes = new Set(source.getPages().map((page) => page.url));

function makeDocumentationLinksMachineReadable(markdown: string) {
	return markdown.replace(
		/\]\((\/[^)#]+)(#[^)]+)?\)/gu,
		(match, path: string, hash: string | undefined) => {
			const normalizedPath = path.replace(/\/$/u, "") || "/";
			if (!documentationRoutes.has(normalizedPath)) return match;
			return `](${normalizedPath}.md${hash ?? ""})`;
		},
	);
}

type DocumentationPage = (typeof source)["$inferPage"];

function getEndpointText(
	page: Extract<DocumentationPage, { type: "openapi" }>,
) {
	const operations = page.data.getOpenAPIPageProps().operations ?? [];
	const endpoints = operations
		.map(
			(operation) =>
				`- \`${operation.method.toUpperCase()} ${operation.path}\``,
		)
		.join("\n");

	return `# ${page.data.title}

${page.data.description ?? ""}

Canonical URL: https://docs.zuse.sh${page.url}

${endpoints}

Request and response schemas: https://docs.zuse.sh/openapi.json`;
}

export async function getLLMText(page: DocumentationPage) {
	if (page.type === "openapi") return getEndpointText(page);

	const markdown = makeDocumentationLinksMachineReadable(
		await page.data.getText("processed"),
	);

	return `# ${page.data.title}

> ${page.data.description}

Canonical URL: https://docs.zuse.sh${page.url}

${markdown}`;
}

import { getLLMText } from "@/lib/get-llm-text";
import { source } from "@/lib/source";

export const dynamic = "force-static";
export const dynamicParams = false;

/** Each page exports as `/api/markdown/<path>.md`; the Worker serves it at `/<path>.md`. */
export function generateStaticParams() {
	return source.generateParams().map(({ slug }) => ({
		slug: [...slug.slice(0, -1), `${slug.at(-1)}.md`],
	}));
}

export async function GET(
	_request: Request,
	context: { params: Promise<{ slug: string[] }> },
) {
	const slug = (await context.params).slug;
	const page = source.getPage([
		...slug.slice(0, -1),
		slug.at(-1)?.replace(/\.md$/u, "") ?? "",
	]);
	if (!page) return new Response("Not found", { status: 404 });

	return new Response(await getLLMText(page), {
		headers: { "Content-Type": "text/markdown; charset=utf-8" },
	});
}

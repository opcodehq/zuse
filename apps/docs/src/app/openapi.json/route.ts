import document from "../../../openapi/zuse-api.json";

export const dynamic = "force-static";

export function GET() {
	return Response.json(document, {
		headers: { "Access-Control-Allow-Origin": "*" },
	});
}

import { pages } from "../llms";

export async function GET() {
  return new Response(await pages("full"), { headers: { "Content-Type": "text/plain; charset=utf-8" } });
}

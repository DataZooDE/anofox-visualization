// #1022: /llms.txt (an index of the docs for language models, llmstxt.org)
// and /llms-full.txt (every page's Markdown in one file), built from the
// docs collection so they follow the site.
import { pages } from "../llms";

export async function GET() {
  return new Response(await pages("index"), { headers: { "Content-Type": "text/plain; charset=utf-8" } });
}

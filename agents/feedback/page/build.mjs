// Bundles the feedback form into one self-contained HTML file,
// page/dist/feedback.html: UI SDK's stylesheet and the page script inline,
// nothing fetched. The app's sandbox blocks every network load, so an html
// artifact must carry everything itself.
//
//   node page/build.mjs
import { build } from "esbuild";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);

const { outputFiles } = await build({
  entryPoints: [join(here, "main.ts")],
  bundle: true,
  minify: true,
  format: "iife",
  target: "safari18",
  write: false,
  legalComments: "none",
});
const js = outputFiles[0].text;
const css = await readFile(
  join(dirname(require.resolve("@ui-sdk/web/package.json")), "browser/web.css"),
  "utf8",
);
const template = await readFile(join(here, "index.html"), "utf8");
// A literal `</script>` in the bundle would end the inline script early.
const html = template
  .replace("/*UI_SDK_CSS*/", () => css)
  .replace("/*PAGE_JS*/", () => js.replaceAll("</script", "<\\/script"));

await mkdir(join(here, "dist"), { recursive: true });
await writeFile(join(here, "dist/feedback.html"), html);
console.log(`page/dist/feedback.html ${(html.length / 1024).toFixed(0)} KB (js ${(js.length / 1024).toFixed(0)} KB, css ${(css.length / 1024).toFixed(0)} KB)`);

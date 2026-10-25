import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { TEAM_MEMBERS_PATH } from "@/lib/commercial";

const read = (path: string) => readFileSync(fileURLToPath(new URL(path, import.meta.url)), "utf8");

function calls(source: string): ts.CallExpression[] {
  const file = ts.createSourceFile("source.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const found: ts.CallExpression[] = [];
  const visit = (node: ts.Node) => {
    if (ts.isCallExpression(node)) found.push(node);
    ts.forEachChild(node, visit);
  };
  visit(file);
  return found;
}

describe("team members path ownership", () => {
  it("commercial owns the path and the page delegates to that client", () => {
    const client = read("../lib/commercial.ts");
    const page = read("../app/settings/team/page.tsx");
    expect(calls(client).some((call) => call.expression.getText() === "request"
      && call.arguments[0]?.getText() === "TEAM_MEMBERS_PATH")).toBe(true);
    expect(calls(page).some((call) => call.expression.getText() === "commercial.members")).toBe(true);
    expect(calls(page).some((call) => ["request", "fetch"].includes(call.expression.getText()))).toBe(false);
    expect(page).not.toContain(TEAM_MEMBERS_PATH);
    expect(page).toContain("res.members");
  });

  it("the current-organization alias is explicitly documented and implemented", () => {
    const spec = JSON.parse(read("../../../../openapi/openapi.json"));
    expect(spec.paths[TEAM_MEMBERS_PATH]?.get?.operationId).toBe("saas.listCurrentOrganizationMembers");
    const routes = read("../../../../crates/server/src/saas/mod.rs");
    const handler = read("../../../../crates/server/src/saas/organizations.rs");
    expect(routes).toContain('"/api/saas/organizations/:id/members"');
    expect(handler).toContain('if id == "current"');
    expect(handler).toContain("AccessRequest::read_only(Permission::UsersRead)");
  });
});

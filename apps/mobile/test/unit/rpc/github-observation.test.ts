import { FolderId } from "@zuse/contracts";
import { emptyPr } from "@zuse/git/github-pr-info";
import { Effect } from "effect";
import { afterEach, expect, test, vi } from "vitest";
import {
	loadGithubObservation,
	resetGithubContext,
} from "../../../src/rpc/github";

const mocks = vi.hoisted(() => ({
	read: vi.fn(),
	write: vi.fn(),
	client: vi.fn(),
	observe: vi.fn(),
	summary: vi.fn(),
	snapshot: vi.fn(),
}));
vi.mock("../../../src/offline/cache", () => ({
	readGithubTarget: mocks.read,
	writeGithubTarget: mocks.write,
}));
vi.mock("../../../src/store/cloud-catalog", () => ({
	cloudSummary: mocks.summary,
}));
vi.mock("../../../src/rpc/connection", () => ({
	getConnectionClient: mocks.client,
	getConnectionSnapshot: mocks.snapshot,
}));
vi.mock("../../../src/rpc/api-client", () => ({
	cloudControlClient: { "cloud.github.observe": mocks.observe },
}));
afterEach(() => {
	resetGithubContext();
	vi.resetAllMocks();
});
const connection = {
	host: "offline",
	port: 1,
	cloudWorkspaceId: "paused-workspace",
};

test("a paused cloud workspace reads GitHub using the account API without opening its connection", async () => {
	mocks.read.mockReturnValue(Effect.succeed(null));
	mocks.summary.mockReturnValue({
		repositoryIdentity: "github.com/acme/app",
		branch: "feature",
	});
	mocks.snapshot.mockReturnValue({ status: "disconnected" });
	mocks.observe.mockImplementation((input) =>
		Effect.succeed({
			pr: { ...emptyPr(input.target.branch), state: "open", number: 42 },
			details: null,
		}),
	);
	const result = await Effect.runPromise(
		loadGithubObservation({ connection, folderId: FolderId.make("folder") }),
	);
	expect(result.pr.number).toBe(42);
	expect(mocks.client).not.toHaveBeenCalled();
	expect(mocks.observe).toHaveBeenCalledWith({
		target: {
			host: "github.com",
			owner: "acme",
			repo: "app",
			branch: "feature",
		},
		force: false,
	});
});

test("retained fork identity is used while the machine is offline", async () => {
	const target = {
		host: "github.com",
		owner: "upstream",
		repo: "app",
		branch: "feature",
		headOwner: "alice",
		headSha: "abc",
	};
	mocks.read.mockReturnValue(Effect.succeed(target));
	mocks.snapshot.mockReturnValue({ status: "disconnected" });
	mocks.observe.mockReturnValue(
		Effect.succeed({ pr: emptyPr("feature"), details: null }),
	);
	await Effect.runPromise(
		loadGithubObservation({ connection, folderId: FolderId.make("folder") }),
	);
	expect(mocks.client).not.toHaveBeenCalled();
	expect(mocks.observe.mock.calls[0]?.[0].target).toEqual(target);
});

test("a hung connected runtime cannot delay GitHub reads with retained identity", async () => {
	const target = {
		host: "github.com",
		owner: "acme",
		repo: "app",
		branch: "feature",
	};
	mocks.read.mockReturnValue(Effect.succeed(target));
	mocks.snapshot.mockReturnValue({ status: "connected" });
	mocks.client.mockReturnValue(
		Effect.succeed({ "git.githubContext": () => Effect.never }),
	);
	mocks.observe.mockReturnValue(
		Effect.succeed({
			pr: { ...emptyPr("feature"), number: 42 },
			details: null,
		}),
	);
	const result = await Effect.runPromise(
		loadGithubObservation({
			connection,
			folderId: FolderId.make("folder"),
		}).pipe(Effect.timeout("1 second")),
	);
	expect(result.pr.number).toBe(42);
	expect(mocks.observe).toHaveBeenCalledOnce();
});

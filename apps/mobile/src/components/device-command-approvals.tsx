import { useCallback, useEffect, useState } from "react";
import { Text, View } from "react-native";
import { apiBaseUrl } from "~/auth/config";
import { getAccessToken } from "~/auth/workos";
import { Button } from "~/components/ui/button";

type Preview = {
	id: string;
	deviceId: string;
	computerName: string;
	role?: string;
	osUser: string;
	shell: string;
	cwd: string;
	command: string;
	timeoutMs: number;
};

type Computer = { deviceId: string; name: string; platform: string; alwaysAllowed: boolean; connected: boolean };

const choices = [
	["once", "Approve once"],
	["session", "Approve for this session"],
	["always", "Approve always"],
	["deny", "Deny"],
] as const;

export function DeviceCommandApprovals(props: { workspaceId: string; chatId: string }) {
	const path = `${apiBaseUrl()}/v1/cloud/workspaces/${encodeURIComponent(props.workspaceId)}/chats/${encodeURIComponent(props.chatId)}/device-approvals`;
	const attachedPath = `${path.slice(0, path.lastIndexOf("/"))}/attached-computers`;
	const taskPath = `${path.slice(0, path.lastIndexOf("/"))}/computer-tasks`;
	const [tasks, setTasks] = useState<{ task: string; computer: string; command: string; cwd: string; dynamic: boolean }[]>([]);
	const [results, setResults] = useState<{ id: string; computer: string; command: string; state: string; result: { status: string; output: string } }[]>([]);
	const [computers, setComputers] = useState<Computer[]>([]);
	const [attached, setAttached] = useState<string[]>([]);
	const [open, setOpen] = useState(false);
	const [requests, setRequests] = useState<Preview[] | null>(null);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState("");
	const [revision, setRevision] = useState(0);

	useEffect(() => {
		if (!open) return;
		const controller = new AbortController();
		let timer: ReturnType<typeof setTimeout>;
		async function refresh() {
			try {
				const token = await getAccessToken();
				const responses = await Promise.all([path, `${apiBaseUrl()}/v1/cloud/bridge/computers`, attachedPath, taskPath, taskPath.replace(/computer-tasks$/, "computer-results")].map((url) => fetch(url, {
					headers: { authorization: `Bearer ${token}` },
					signal: controller.signal,
				})));
				if (responses.some((response) => !response.ok)) throw new Error();
				const [rows, devices, ids, available, finished] = await Promise.all(responses.map((response) => response.json()));
				if (!controller.signal.aborted) {
					setTasks(available);
					setResults(finished);
					setRequests(rows);
					setComputers(devices);
					setAttached((ids as { deviceId: string }[]).map((row) => row.deviceId));
					setError((previous) => previous === "Could not load command approvals." ? "" : previous);
				}
			} catch {
				if (!controller.signal.aborted) {
					setTasks([]);
					setResults([]);
					setRequests(null);
					setComputers([]);
					setAttached([]);
					setError("Could not load command approvals.");
				}
			} finally {
				if (!controller.signal.aborted) timer = setTimeout(refresh, 5000);
			}
		}
		void refresh();
		return () => {
			controller.abort();
			clearTimeout(timer);
		};
	}, [path, attachedPath, taskPath, open, revision]);

	const decide = useCallback(async (id: string, decision: typeof choices[number][0]) => {
		setBusy(true);
		setError("");
		try {
			const token = await getAccessToken();
			const response = await fetch(`${path}/${encodeURIComponent(id)}`, {
				method: "POST",
				headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
				body: JSON.stringify({ decision }),
			});
			if (!response.ok) throw new Error();
			setRequests((rows) => (rows ?? []).filter((row) => row.id !== id));
		} catch {
			setError("Could not confirm the decision. Check the refreshed command list.");
		} finally {
			setRevision((value) => value + 1);
			setBusy(false);
		}
	}, [path]);

	async function cancelRun(id:string) {
		setBusy(true);setError("");
		try {
			const token=await getAccessToken();
			const response=await fetch(`${taskPath.replace(/computer-tasks$/, "computer-results")}/${encodeURIComponent(id)}`,{
				method:"DELETE",headers:{authorization:`Bearer ${token}`},
			});
			if(!response.ok)throw new Error();
		}catch{setError("Could not confirm cancellation.");}
		finally{setBusy(false);setRevision(value=>value+1);}
	}

	async function requestTask(task: string) {
		setBusy(true);
		setError("");
		try {
			const token = await getAccessToken();
			const response = await fetch(taskPath, {
				method: "POST", headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
				body: JSON.stringify({ task, request_id: crypto.randomUUID() }),
			});
			if (!response.ok) throw new Error();
		} catch {
			setError("Could not confirm task submission. Check pending approvals and results.");
		} finally {
			setRevision((value) => value + 1);
			setBusy(false);
		}
	}

	async function revokeAlways(id: string) {
		setBusy(true);
		setError("");
		try {
			const token = await getAccessToken();
			const response = await fetch(`${apiBaseUrl()}/v1/cloud/bridge/computers/${encodeURIComponent(id)}/always-allow`, {
				method: "DELETE",
				headers: { authorization: `Bearer ${token}` },
			});
			if (!response.ok) throw new Error();
			setComputers((rows) => rows.map((row) => row.deviceId === id ? { ...row, alwaysAllowed: false } : row));
		} catch {
			setError("Could not revoke access.");
		} finally {
			setRevision((value) => value + 1);
			setBusy(false);
		}
	}

	return (
		<View className="gap-2 p-3">
			<Button size="sm" onPress={() => { setError(""); setOpen(!open); }}>
				{open ? "Hide computers" : "Computers"}
			</Button>
			{open && computers.map((computer) => (
				<View key={computer.deviceId} className="gap-1">
				<Text>{computer.name} · {computer.platform} · {computer.connected ? "Online" : "Offline"}</Text>
				{computer.alwaysAllowed && <Button size="sm" disabled={busy} onPress={() => void revokeAlways(computer.deviceId)}>
					Revoke always allow for {computer.name}
				</Button>}
				</View>
			))}
			{open && error && <Text role="alert">{error}</Text>}
			{open && !error && requests === null && <Text>Loading approvals…</Text>}
			{open && !error && requests?.length === 0 && <Text>No commands awaiting approval.</Text>}
			{open && tasks.filter(task=>!task.dynamic).map((task) => (
				<View key={task.task}>
					<Text selectable>{task.command}{"\n"}{task.cwd}</Text>
					<Button size="sm" disabled={busy} onPress={() => void requestTask(task.task)}>Run on {task.computer}</Button>
				</View>
			))}
			{open && results.filter((run) => run.state !== "pending").map((run) => (
				<View key={run.id}>
					<Text>{run.computer} · {run.result.status}</Text>
					{(run.state === "running" || run.state === "approved") && <Button disabled={busy} size="sm" onPress={()=>void cancelRun(run.id)}>Stop</Button>}
					<Text selectable className="font-mono">{run.command}{"\n"}{run.result.output}</Text>
				</View>
			))}
			{open && requests?.map((row) => (
				<View key={row.id} className="gap-2">
					<Text>{row.computerName} · {row.osUser} · {row.shell}</Text>
					<Text selectable className="font-mono">{row.cwd}{"\n"}{row.command}</Text>
					<Text>Runs with this OS user's access to files and credentials. Always allows your future chats to use this computer.</Text>
					<View className="flex-row flex-wrap gap-1">
						{choices.map(([decision, label]) => (
							<Button key={decision} size="sm" disabled={busy} onPress={() => void decide(row.id, decision)}>
								{label}
							</Button>
						))}
					</View>
				</View>
			))}
		</View>
	);
}

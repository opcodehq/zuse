type RecorderCleanupTarget = {
	stop: () => Promise<void>;
};

export const stopRecorderOnUnmount = async (
	recorder: RecorderCleanupTarget,
): Promise<void> => {
	try {
		await recorder.stop();
	} catch {}
};

export type LogLevel = 'debug' | 'info' | 'warn' | 'error';

const LOG_LEVELS: Record<LogLevel, number> = {
	debug: 0,
	info: 1,
	warn: 2,
	error: 3,
};

let currentLogLevel: LogLevel = 'info';

export function setLogLevel(level: LogLevel): void {
	currentLogLevel = level;
}

function shouldLog(level: LogLevel): boolean {
	return LOG_LEVELS[level] >= LOG_LEVELS[currentLogLevel];
}

function formatMessage(level: LogLevel, namespace: string, message: string, meta?: Record<string, unknown>): string {
	const timestamp = new Date().toISOString();
	const metaStr = meta ? ` ${JSON.stringify(meta)}` : '';
	return `[${timestamp}] ${level.toUpperCase()} [${namespace}] ${message}${metaStr}`;
}

interface Logger {
	debug(message: string, meta?: Record<string, unknown>): void;
	info(message: string, meta?: Record<string, unknown>): void;
	warn(message: string, meta?: Record<string, unknown>): void;
	error(message: string, meta?: Record<string, unknown>): void;
}

export function createLogger(namespace: string): Logger {
	return {
		debug(message: string, meta?: Record<string, unknown>): void {
			if (shouldLog('debug')) {
				console.debug(formatMessage('debug', namespace, message, meta));
			}
		},
		info(message: string, meta?: Record<string, unknown>): void {
			if (shouldLog('info')) {
				console.info(formatMessage('info', namespace, message, meta));
			}
		},
		warn(message: string, meta?: Record<string, unknown>): void {
			if (shouldLog('warn')) {
				console.warn(formatMessage('warn', namespace, message, meta));
			}
		},
		error(message: string, meta?: Record<string, unknown>): void {
			if (shouldLog('error')) {
				console.error(formatMessage('error', namespace, message, meta));
			}
		},
	};
}

export function createChildLogger(parent: Logger, extra: Record<string, unknown>): Logger {
	return {
		debug(message: string, meta?: Record<string, unknown>): void {
			parent.debug(message, { ...extra, ...meta });
		},
		info(message: string, meta?: Record<string, unknown>): void {
			parent.info(message, { ...extra, ...meta });
		},
		warn(message: string, meta?: Record<string, unknown>): void {
			parent.warn(message, { ...extra, ...meta });
		},
		error(message: string, meta?: Record<string, unknown>): void {
			parent.error(message, { ...extra, ...meta });
		},
	};
}

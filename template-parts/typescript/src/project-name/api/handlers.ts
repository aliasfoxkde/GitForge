import type { ApiResponse, HealthResponse, CreateResourceRequest, Resource } from '../models/types.js';
import { BusinessService } from '../services/business.js';
import { createLogger } from '../utils/logging.js';

const logger = createLogger('api');

const businessService = new BusinessService();

export function registerHandlers(): void {
	logger.info('Registering API handlers');
}

export async function handleHealth(): Promise<HealthResponse> {
	const status = await businessService.checkHealth();
	return {
		status,
		timestamp: new Date().toISOString(),
		version: process.env.npm_package_version ?? '1.0.0',
	};
}

export async function handleListResources(): Promise<ApiResponse<Resource[]>> {
	try {
		const resources = await businessService.listResources();
		return {
			success: true,
			data: resources,
		};
	} catch (error) {
		logger.error('Failed to list resources', { error });
		return {
			success: false,
			error: error instanceof Error ? error.message : 'Unknown error',
		};
	}
}

export async function handleCreateResource(
	request: CreateResourceRequest,
): Promise<ApiResponse<Resource>> {
	try {
		if (!request.name || request.name.trim().length === 0) {
			return {
				success: false,
				error: 'Resource name is required',
			};
		}
		const resource = await businessService.createResource(request);
		return {
			success: true,
			data: resource,
		};
	} catch (error) {
		logger.error('Failed to create resource', { error, request });
		return {
			success: false,
			error: error instanceof Error ? error.message : 'Unknown error',
		};
	}
}

export async function handleGetResource(id: string): Promise<ApiResponse<Resource>> {
	try {
		if (!id) {
			return {
				success: false,
				error: 'Resource ID is required',
			};
		}
		const resource = await businessService.getResource(id);
		if (!resource) {
			return {
				success: false,
				error: `Resource not found: ${id}`,
			};
		}
		return {
			success: true,
			data: resource,
		};
	} catch (error) {
		logger.error('Failed to get resource', { error, id });
		return {
			success: false,
			error: error instanceof Error ? error.message : 'Unknown error',
		};
	}
}

export async function handleDeleteResource(id: string): Promise<ApiResponse<void>> {
	try {
		if (!id) {
			return {
				success: false,
				error: 'Resource ID is required',
			};
		}
		const deleted = await businessService.deleteResource(id);
		if (!deleted) {
			return {
				success: false,
				error: `Resource not found: ${id}`,
			};
		}
		return {
			success: true,
		};
	} catch (error) {
		logger.error('Failed to delete resource', { error, id });
		return {
			success: false,
			error: error instanceof Error ? error.message : 'Unknown error',
		};
	}
}

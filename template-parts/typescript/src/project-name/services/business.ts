import { randomUUID } from 'node:crypto';
import type {
	Resource,
	CreateResourceRequest,
	UpdateResourceRequest,
	HealthStatus,
} from '../models/types.js';
import { createLogger } from '../utils/logging.js';

const logger = createLogger('service');

export class BusinessService {
	private readonly resources: Map<string, Resource> = new Map();

	async checkHealth(): Promise<HealthStatus> {
		logger.debug('Checking health status');
		return 'healthy';
	}

	async listResources(): Promise<Resource[]> {
		logger.debug('Listing all resources');
		return Array.from(this.resources.values());
	}

	async getResource(id: string): Promise<Resource | undefined> {
		logger.debug('Getting resource', { id });
		return this.resources.get(id);
	}

	async createResource(request: CreateResourceRequest): Promise<Resource> {
		const id = randomUUID();
		const now = new Date().toISOString();

		const resource: Resource = {
			id,
			name: request.name.trim(),
			description: request.description?.trim(),
			tags: request.tags ?? [],
			metadata: request.metadata ?? {},
			createdAt: now,
			updatedAt: now,
		};

		this.resources.set(id, resource);
		logger.info('Resource created', { id, name: resource.name });

		return resource;
	}

	async updateResource(id: string, request: UpdateResourceRequest): Promise<Resource | undefined> {
		const existing = this.resources.get(id);
		if (!existing) {
			return undefined;
		}

		const updated: Resource = {
			...existing,
			name: request.name?.trim() ?? existing.name,
			description: request.description?.trim() ?? existing.description,
			tags: request.tags ?? existing.tags,
			metadata: request.metadata ?? existing.metadata,
			updatedAt: new Date().toISOString(),
		};

		this.resources.set(id, updated);
		logger.info('Resource updated', { id, name: updated.name });

		return updated;
	}

	async deleteResource(id: string): Promise<boolean> {
		const deleted = this.resources.delete(id);
		if (deleted) {
			logger.info('Resource deleted', { id });
		}
		return deleted;
	}

	async countResources(): Promise<number> {
		return this.resources.size;
	}

	async clearAllResources(): Promise<void> {
		this.resources.clear();
		logger.warn('All resources cleared');
	}
}

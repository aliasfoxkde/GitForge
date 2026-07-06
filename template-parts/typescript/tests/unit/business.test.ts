import { describe, it, expect, beforeEach, vi } from 'vitest';
import { BusinessService } from '../../src/project-name/services/business.js';
import type { CreateResourceRequest } from '../../src/project-name/models/types.js';

describe('BusinessService', () => {
	let service: BusinessService;

	beforeEach(() => {
		service = new BusinessService();
	});

	describe('checkHealth', () => {
		it('should return healthy status', async () => {
			const status = await service.checkHealth();
			expect(status).toBe('healthy');
		});
	});

	describe('createResource', () => {
		it('should create a resource with the given name', async () => {
			const request: CreateResourceRequest = {
				name: 'Test Resource',
				description: 'A test resource',
				tags: ['test', 'unit'],
			};

			const resource = await service.createResource(request);

			expect(resource).toBeDefined();
			expect(resource.id).toBeTruthy();
			expect(resource.name).toBe('Test Resource');
			expect(resource.description).toBe('A test resource');
			expect(resource.tags).toEqual(['test', 'unit']);
			expect(resource.createdAt).toBeTruthy();
			expect(resource.updatedAt).toBeTruthy();
		});

		it('should trim whitespace from resource name', async () => {
			const request: CreateResourceRequest = {
				name: '  Spaced Name  ',
			};

			const resource = await service.createResource(request);
			expect(resource.name).toBe('Spaced Name');
		});

		it('should generate unique IDs for each resource', async () => {
			const request: CreateResourceRequest = { name: 'Resource' };

			const resource1 = await service.createResource(request);
			const resource2 = await service.createResource(request);

			expect(resource1.id).not.toBe(resource2.id);
		});
	});

	describe('listResources', () => {
		it('should return empty array when no resources exist', async () => {
			const resources = await service.listResources();
			expect(resources).toEqual([]);
		});

		it('should return all created resources', async () => {
			await service.createResource({ name: 'Resource 1' });
			await service.createResource({ name: 'Resource 2' });

			const resources = await service.listResources();
			expect(resources).toHaveLength(2);
		});
	});

	describe('getResource', () => {
		it('should return undefined for non-existent resource', async () => {
			const resource = await service.getResource('non-existent-id');
			expect(resource).toBeUndefined();
		});

		it('should return the created resource by ID', async () => {
			const created = await service.createResource({ name: 'Find Me' });

			const found = await service.getResource(created.id);
			expect(found).toBeDefined();
			expect(found?.name).toBe('Find Me');
		});
	});

	describe('deleteResource', () => {
		it('should return false when deleting non-existent resource', async () => {
			const result = await service.deleteResource('non-existent-id');
			expect(result).toBe(false);
		});

		it('should return true and remove resource when deleted', async () => {
			const created = await service.createResource({ name: 'To Delete' });

			const result = await service.deleteResource(created.id);
			expect(result).toBe(true);

			const found = await service.getResource(created.id);
			expect(found).toBeUndefined();
		});
	});

	describe('countResources', () => {
		it('should return zero for new service', async () => {
			const count = await service.countResources();
			expect(count).toBe(0);
		});

		it('should return correct count after creating resources', async () => {
			await service.createResource({ name: 'Resource 1' });
			await service.createResource({ name: 'Resource 2' });

			const count = await service.countResources();
			expect(count).toBe(2);
		});
	});

	describe('clearAllResources', () => {
		it('should remove all resources', async () => {
			await service.createResource({ name: 'Resource 1' });
			await service.createResource({ name: 'Resource 2' });

			await service.clearAllResources();

			const count = await service.countResources();
			expect(count).toBe(0);
		});
	});

	describe('updateResource', () => {
		it.skip('TODO: implement update resource functionality', async () => {
			// Will be implemented in future iteration
			const created = await service.createResource({ name: 'Original' });
			const updated = await service.updateResource(created.id, { name: 'Updated' });
			expect(updated?.name).toBe('Updated');
		});
	});
});

import { describe, it, expect, beforeEach } from 'vitest';
import {
	handleHealth,
	handleListResources,
	handleCreateResource,
	handleGetResource,
	handleDeleteResource,
	registerHandlers,
} from '../../src/project-name/api/handlers.js';
import type { CreateResourceRequest } from '../../src/project-name/models/types.js';

describe('API Handlers', () => {
	beforeEach(() => {
		registerHandlers();
	});

	describe('handleHealth', () => {
		it('should return health status with timestamp and version', async () => {
			const response = await handleHealth();

			expect(response).toBeDefined();
			expect(response.status).toBeTruthy();
			expect(response.timestamp).toBeTruthy();
			expect(response.version).toBeTruthy();
		});

		it('should return valid ISO timestamp', async () => {
			const response = await handleHealth();

			const timestamp = new Date(response.timestamp);
			expect(timestamp.getTime()).not.toBeNaN();
		});
	});

	describe('handleListResources', () => {
		it('should return success response with empty array initially', async () => {
			const response = await handleListResources();

			expect(response.success).toBe(true);
			expect(response.data).toEqual([]);
		});

		it('should return resources after creation', async () => {
			await handleCreateResource({ name: 'Test Resource' });

			const response = await handleListResources();

			expect(response.success).toBe(true);
			expect(response.data).toHaveLength(1);
			expect(response.data?.[0].name).toBe('Test Resource');
		});
	});

	describe('handleCreateResource', () => {
		it('should create resource with valid request', async () => {
			const request: CreateResourceRequest = {
				name: 'New Resource',
				description: 'Integration test resource',
				tags: ['integration'],
			};

			const response = await handleCreateResource(request);

			expect(response.success).toBe(true);
			expect(response.data).toBeDefined();
			expect(response.data?.name).toBe('New Resource');
			expect(response.data?.id).toBeTruthy();
		});

		it('should fail with empty name', async () => {
			const request: CreateResourceRequest = {
				name: '',
			};

			const response = await handleCreateResource(request);

			expect(response.success).toBe(false);
			expect(response.error).toContain('required');
		});

		it('should fail with whitespace-only name', async () => {
			const request: CreateResourceRequest = {
				name: '   ',
			};

			const response = await handleCreateResource(request);

			expect(response.success).toBe(false);
		});
	});

	describe('handleGetResource', () => {
		it('should return error for missing ID', async () => {
			const response = await handleGetResource('');

			expect(response.success).toBe(false);
			expect(response.error).toContain('required');
		});

		it('should return error for non-existent resource', async () => {
			const response = await handleGetResource('non-existent-id');

			expect(response.success).toBe(false);
			expect(response.error).toContain('not found');
		});

		it('should return resource when found', async () => {
			const createResponse = await handleCreateResource({ name: 'Findable' });
			const resource = createResponse.data;

			const response = await handleGetResource(resource!.id);

			expect(response.success).toBe(true);
			expect(response.data?.name).toBe('Findable');
		});
	});

	describe('handleDeleteResource', () => {
		it('should return error for missing ID', async () => {
			const response = await handleDeleteResource('');

			expect(response.success).toBe(false);
			expect(response.error).toContain('required');
		});

		it('should return error for non-existent resource', async () => {
			const response = await handleDeleteResource('non-existent-id');

			expect(response.success).toBe(false);
			expect(response.error).toContain('not found');
		});

		it('should successfully delete existing resource', async () => {
			const createResponse = await handleCreateResource({ name: 'Deletable' });
			const resource = createResponse.data;

			const response = await handleDeleteResource(resource!.id);

			expect(response.success).toBe(true);
		});
	});
});

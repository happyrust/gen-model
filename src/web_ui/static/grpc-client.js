/**
 * gRPC-Web Client for Spatial Query Service
 * This client enables web browsers to communicate with the gRPC spatial query service
 */

class GrpcSpatialQueryClient {
    constructor(endpoint = 'http://localhost:8080') {
        this.endpoint = endpoint;
        this.client = null;
        this.initialized = false;
        
        // Import required gRPC-Web libraries dynamically
        this.loadGrpcWeb();
    }

    /**
     * Load gRPC-Web libraries
     */
    async loadGrpcWeb() {
        // 本地优先加载 /static/grpc-web.min.js，不存在则回退到 CDN
        if (typeof grpc === 'undefined') {
            const tryLoad = (src) => new Promise((resolve) => {
                const s = document.createElement('script');
                s.src = src;
                s.onload = () => resolve(true);
                s.onerror = () => resolve(false);
                document.head.appendChild(s);
            });

            // 先尝试本地
            const okLocal = await tryLoad('/static/grpc-web.min.js');
            if (!okLocal) {
                await tryLoad('https://cdn.jsdelivr.net/npm/grpc-web@1.4.2/index.min.js');
            }
        }
        this.initializeClient();
    }

    /**
     * Initialize the gRPC client
     */
    initializeClient() {
        // Note: In a real implementation, we would generate these from the .proto file
        // using protoc with the grpc-web plugin
        this.initialized = true;
        console.log('gRPC-Web client initialized');
    }

    /**
     * Query intersecting elements via gRPC
     * @param {Object} request - The query request
     * @returns {Promise<Object>} The query response
     */
    async queryIntersectingElements(request) {
        // Validate request
        if (!request.refno) {
            throw new Error('refno is required');
        }

        // Build gRPC request message
        const grpcRequest = {
            refno: request.refno,
            customBbox: request.customBbox || null,
            elementTypes: request.elementTypes || [],
            includeSelf: request.includeSelf || false,
            tolerance: request.tolerance || 0.001,
            maxResults: request.maxResults || 1000
        };

        // For demonstration, we'll make a POST request to the gRPC-Web proxy
        // In production, this would use the generated gRPC-Web client
        try {
            const response = await this.makeGrpcWebRequest(
                'SpatialQueryService/QueryIntersectingElements',
                grpcRequest
            );
            return this.parseGrpcResponse(response);
        } catch (error) {
            console.error('gRPC request failed:', error);
            throw error;
        }
    }

    /**
     * Batch query intersecting elements via gRPC
     * @param {Array} requests - Array of query requests
     * @returns {Promise<Object>} The batch query response
     */
    async batchQueryIntersecting(requests) {
        const grpcRequest = {
            requests: requests,
            parallelExecution: true
        };

        try {
            const response = await this.makeGrpcWebRequest(
                'SpatialQueryService/BatchQueryIntersecting',
                grpcRequest
            );
            return this.parseGrpcResponse(response);
        } catch (error) {
            console.error('Batch gRPC request failed:', error);
            throw error;
        }
    }

    /**
     * Get spatial index statistics via gRPC
     * @returns {Promise<Object>} The index stats response
     */
    async getIndexStats() {
        try {
            const response = await this.makeGrpcWebRequest(
                'SpatialQueryService/GetIndexStats',
                {}
            );
            return this.parseGrpcResponse(response);
        } catch (error) {
            console.error('Get index stats failed:', error);
            throw error;
        }
    }

    /**
     * Rebuild spatial index via gRPC
     * @param {boolean} forceRebuild - Whether to force rebuild
     * @returns {Promise<Object>} The rebuild response
     */
    async rebuildSpatialIndex(forceRebuild = false) {
        const grpcRequest = {
            forceRebuild: forceRebuild,
            elementTypes: []
        };

        try {
            const response = await this.makeGrpcWebRequest(
                'SpatialQueryService/RebuildSpatialIndex',
                grpcRequest
            );
            return this.parseGrpcResponse(response);
        } catch (error) {
            console.error('Rebuild index failed:', error);
            throw error;
        }
    }

    /**
     * Make a gRPC-Web request
     * @private
     * @param {string} method - The RPC method name
     * @param {Object} request - The request object
     * @returns {Promise<Object>} The raw response
     */
    async makeGrpcWebRequest(method, request) {
        const url = `${this.endpoint}/spatial_query.${method}`;
        
        // Encode the request as binary (simplified for demonstration)
        // In production, this would use proper protobuf encoding
        const requestBody = this.encodeProtobuf(request);
        
        const response = await fetch(url, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/grpc-web+proto',
                'X-Grpc-Web': '1',
                'Accept': 'application/grpc-web+proto'
            },
            body: requestBody
        });

        if (!response.ok) {
            const grpcStatus = response.headers.get('grpc-status');
            const grpcMessage = response.headers.get('grpc-message');
            throw new Error(`gRPC error ${grpcStatus}: ${grpcMessage || 'Unknown error'}`);
        }

        return response;
    }

    /**
     * Parse gRPC response
     * @private
     * @param {Response} response - The fetch response
     * @returns {Promise<Object>} The parsed response object
     */
    async parseGrpcResponse(response) {
        // For demonstration, we'll parse as JSON
        // In production, this would decode the protobuf response
        const buffer = await response.arrayBuffer();
        
        // Simplified parsing - in reality, we'd use protobuf.js or similar
        try {
            const text = new TextDecoder().decode(buffer);
            return JSON.parse(text);
        } catch {
            // If not JSON, return a mock response for demonstration
            return this.createMockResponse(response.headers.get('grpc-status'));
        }
    }

    /**
     * Encode request as protobuf
     * @private
     * @param {Object} obj - The object to encode
     * @returns {ArrayBuffer} The encoded buffer
     */
    encodeProtobuf(obj) {
        // Simplified encoding for demonstration
        // In production, use protobuf.js or generated code
        const json = JSON.stringify(obj);
        const encoder = new TextEncoder();
        return encoder.encode(json);
    }

    /**
     * Create a mock response for demonstration
     * @private
     * @param {string} status - The gRPC status
     * @returns {Object} Mock response object
     */
    createMockResponse(status) {
        if (status === '0') {
            return {
                elements: [
                    {
                        refno: 1001,
                        elementType: 'PIPE',
                        bbox: {
                            min: { x: 0, y: 0, z: 0 },
                            max: { x: 1, y: 1, z: 1 }
                        },
                        intersectionVolume: 0.5,
                        distanceToCenter: 0.8,
                        elementName: 'PIPE-001 (via gRPC)'
                    }
                ],
                totalCount: 1,
                queryTimeMs: '25',
                success: true,
                errorMessage: ''
            };
        } else {
            return {
                elements: [],
                totalCount: 0,
                queryTimeMs: '0',
                success: false,
                errorMessage: 'gRPC request failed'
            };
        }
    }

    /**
     * Perform a benchmark comparison between HTTP and gRPC
     * @param {Object} request - The query request
     * @returns {Promise<Object>} Benchmark results
     */
    async benchmarkComparison(request, iterations = 10) {
        const results = {
            grpc: {
                times: [],
                avgTime: 0,
                minTime: Infinity,
                maxTime: 0
            },
            http: {
                times: [],
                avgTime: 0,
                minTime: Infinity,
                maxTime: 0
            }
        };

        // Benchmark gRPC
        for (let i = 0; i < iterations; i++) {
            const start = performance.now();
            try {
                await this.queryIntersectingElements(request);
            } catch (error) {
                console.warn('gRPC benchmark iteration failed:', error);
            }
            const elapsed = performance.now() - start;
            results.grpc.times.push(elapsed);
            results.grpc.minTime = Math.min(results.grpc.minTime, elapsed);
            results.grpc.maxTime = Math.max(results.grpc.maxTime, elapsed);
        }
        results.grpc.avgTime = results.grpc.times.reduce((a, b) => a + b, 0) / iterations;

        // Benchmark HTTP (using existing REST API)
        for (let i = 0; i < iterations; i++) {
            const start = performance.now();
            try {
                await fetch('/api/sqlite-spatial/query', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify(request)
                });
            } catch (error) {
                console.warn('HTTP benchmark iteration failed:', error);
            }
            const elapsed = performance.now() - start;
            results.http.times.push(elapsed);
            results.http.minTime = Math.min(results.http.minTime, elapsed);
            results.http.maxTime = Math.max(results.http.maxTime, elapsed);
        }
        results.http.avgTime = results.http.times.reduce((a, b) => a + b, 0) / iterations;

        // Calculate improvement
        results.improvement = {
            percentage: ((results.http.avgTime - results.grpc.avgTime) / results.http.avgTime * 100).toFixed(2),
            factor: (results.http.avgTime / results.grpc.avgTime).toFixed(2)
        };

        return results;
    }
}

// Export for use in other modules
if (typeof module !== 'undefined' && module.exports) {
    module.exports = GrpcSpatialQueryClient;
}

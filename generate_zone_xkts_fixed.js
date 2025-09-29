#!/usr/bin/env node

import fetch from 'node-fetch';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

/**
 * Generate XKT files for zones with known valid refnos
 */
class ZoneXKTGenerator {
    constructor(baseUrl = 'http://localhost:8080') {
        this.baseUrl = baseUrl;
        this.dbno = 1112;
        this.outputDir = path.join('output', 'zones', `db${this.dbno}`);
        this.zones = [];
        this.generatedFiles = [];
    }

    /**
     * Define zones with ONLY verified refnos
     */
    defineZones() {
        // Only use refnos we've successfully tested
        this.zones = [
            {
                id: 'zone_001',
                name: 'Process Area A',
                refno: '17496/266203',
                description: 'Main process equipment area - verified'
            }
        ];

        // Try to find more valid refnos by scanning a range
        const baseRefno = 17496;
        for (let i = 266204; i <= 266210; i++) {
            this.zones.push({
                id: `zone_${String(this.zones.length + 1).padStart(3, '0')}`,
                name: `Zone ${this.zones.length + 1}`,
                refno: `${baseRefno}/${i}`,
                description: `Test zone with refno ${baseRefno}/${i}`
            });
        }

        console.log(`📋 Defined ${this.zones.length} zones for generation`);
    }

    /**
     * Test if a refno has valid data
     */
    async testRefno(refno) {
        try {
            const response = await fetch(`${this.baseUrl}/api/xkt/generate`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    dbno: this.dbno,
                    refno: refno,
                    compress: true
                }),
                timeout: 10000  // 10 second timeout
            });

            if (!response.ok) {
                return false;
            }

            const result = await response.json();
            return result.success === true;
        } catch (error) {
            return false;
        }
    }

    /**
     * Find valid zones by testing refnos
     */
    async findValidZones() {
        console.log('🔍 Finding zones with valid geometry data...\n');
        const validZones = [];

        for (const zone of this.zones) {
            process.stdout.write(`Testing ${zone.refno}... `);
            const isValid = await this.testRefno(zone.refno);

            if (isValid) {
                console.log('✅ Valid');
                validZones.push(zone);
            } else {
                console.log('❌ No data');
            }

            // Small delay to avoid overwhelming the server
            await new Promise(resolve => setTimeout(resolve, 500));
        }

        this.zones = validZones;
        console.log(`\n✅ Found ${this.zones.length} valid zones`);
    }

    /**
     * Generate XKT for a single zone with timeout
     */
    async generateZoneXKT(zone, compress = true) {
        console.log(`\n🔧 Generating XKT for ${zone.name} (${zone.refno})...`);

        try {
            // Set a timeout for the fetch request
            const controller = new AbortController();
            const timeout = setTimeout(() => controller.abort(), 30000); // 30 second timeout

            // Call the XKT generation API
            const response = await fetch(`${this.baseUrl}/api/xkt/generate`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    dbno: this.dbno,
                    refno: zone.refno,
                    compress: compress
                }),
                signal: controller.signal
            });

            clearTimeout(timeout);

            if (!response.ok) {
                const text = await response.text();
                throw new Error(`API error ${response.status}: ${text}`);
            }

            const result = await response.json();

            if (result.success) {
                console.log(`✅ Generated: ${result.filename}`);

                // Download the file
                const downloadUrl = `${this.baseUrl}${result.url}`;
                const xktResponse = await fetch(downloadUrl);

                if (!xktResponse.ok) {
                    throw new Error(`Failed to download XKT file: ${xktResponse.statusText}`);
                }

                const xktData = await xktResponse.arrayBuffer();
                const buffer = Buffer.from(xktData);

                // Save to zone output directory
                const zoneFilename = `${zone.id}.xkt`;
                const zonePath = path.join(this.outputDir, zoneFilename);
                fs.writeFileSync(zonePath, buffer);

                const fileSize = buffer.length;
                console.log(`  📁 Saved to: ${zonePath}`);
                console.log(`  📊 File size: ${fileSize.toLocaleString()} bytes`);

                return {
                    zone: zone,
                    filename: zoneFilename,
                    path: zonePath,
                    size: fileSize,
                    compressed: compress,
                    generatedAt: new Date().toISOString()
                };
            } else {
                throw new Error('Generation failed: ' + JSON.stringify(result));
            }
        } catch (error) {
            if (error.name === 'AbortError') {
                console.error(`❌ Request timeout for ${zone.name}`);
            } else {
                console.error(`❌ Failed to generate XKT for ${zone.name}: ${error.message}`);
            }
            return null;
        }
    }

    /**
     * Validate generated XKT file
     */
    async validateXKT(filePath) {
        try {
            const buffer = fs.readFileSync(filePath);

            if (buffer.length < 120) {
                throw new Error('File too small to be valid XKT');
            }

            // Read version and compression flag
            const versionAndFlags = buffer.readUInt32LE(0);
            const version = versionAndFlags & 0x7FFFFFFF;
            const compressed = (versionAndFlags & 0x80000000) !== 0;

            // Read section offsets
            const sectionOffsets = [];
            for (let i = 0; i < 29; i++) {
                const offset = buffer.readUInt32LE(4 + i * 4);
                sectionOffsets.push(offset);
            }

            // Check if file has actual data
            const hasData = sectionOffsets.some(offset => offset > 120);

            return {
                valid: hasData,
                version: version,
                compressed: compressed,
                size: buffer.length,
                hasGeometry: hasData
            };
        } catch (error) {
            return {
                valid: false,
                error: error.message
            };
        }
    }

    /**
     * Generate zone manifest
     */
    async generateManifest() {
        console.log('\n📝 Generating zone manifest...');

        const manifest = {
            database: this.dbno,
            totalZones: this.generatedFiles.length,
            generatedAt: new Date().toISOString(),
            zones: []
        };

        for (const file of this.generatedFiles) {
            if (!file) continue;

            // Validate XKT file
            const validation = await this.validateXKT(file.path);

            // Zone info
            const zoneInfo = {
                id: file.zone.id,
                name: file.zone.name,
                refno: file.zone.refno,
                description: file.zone.description,
                xktFile: file.filename,
                fileSize: file.size,
                compressed: file.compressed,
                hasGeometry: validation.hasGeometry,
                valid: validation.valid,
                generatedAt: file.generatedAt,
                // Placeholder bounding box (would need actual calculation)
                boundingBox: {
                    min: [-100, -100, 0],
                    max: [100, 100, 200]
                },
                center: [0, 0, 100],
                radius: 173.2
            };

            manifest.zones.push(zoneInfo);
        }

        // Save manifest
        const manifestPath = path.join(this.outputDir, 'zone_manifest.json');
        fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

        console.log(`✅ Manifest saved to: ${manifestPath}`);
        return manifest;
    }

    /**
     * Main execution
     */
    async run() {
        console.log('='.repeat(60));
        console.log('🏗️  Zone-Based XKT Generation (Fixed)');
        console.log('='.repeat(60));

        // Create output directory
        if (!fs.existsSync(this.outputDir)) {
            fs.mkdirSync(this.outputDir, { recursive: true });
            console.log(`📁 Created output directory: ${this.outputDir}`);
        }

        // Define zones
        this.defineZones();

        // Find valid zones
        await this.findValidZones();

        if (this.zones.length === 0) {
            console.log('⚠️ No valid zones found. Exiting.');
            return null;
        }

        // Generate XKT for each valid zone
        console.log(`\n🚀 Starting XKT generation for ${this.zones.length} valid zones...`);

        for (const zone of this.zones) {
            const result = await this.generateZoneXKT(zone, true);

            if (result) {
                this.generatedFiles.push(result);

                // Validate the generated file
                const validation = await this.validateXKT(result.path);
                if (validation.valid && validation.hasGeometry) {
                    console.log(`  ✅ Validation passed: Has geometry`);
                } else if (validation.valid && !validation.hasGeometry) {
                    console.log(`  ⚠️ Valid XKT but no geometry data`);
                } else {
                    console.log(`  ❌ Validation failed: ${validation.error}`);
                }
            }

            // Small delay to avoid overwhelming the server
            await new Promise(resolve => setTimeout(resolve, 1000));
        }

        // Generate manifest
        const manifest = await this.generateManifest();

        // Summary
        console.log('\n' + '='.repeat(60));
        console.log('📊 Generation Summary:');
        console.log('='.repeat(60));
        console.log(`Total zones attempted: ${this.zones.length}`);
        console.log(`Successfully generated: ${this.generatedFiles.filter(f => f !== null).length}`);
        console.log(`Failed: ${this.zones.length - this.generatedFiles.filter(f => f !== null).length}`);

        let totalSize = 0;
        this.generatedFiles.forEach(file => {
            if (file) totalSize += file.size;
        });
        console.log(`Total size: ${(totalSize / 1024).toFixed(2)} KB`);

        console.log('\n✅ Zone XKT generation complete!');
        console.log(`📁 Files saved to: ${this.outputDir}`);

        return manifest;
    }
}

// Run if executed directly
if (import.meta.url === `file://${process.argv[1]}`) {
    const generator = new ZoneXKTGenerator();
    generator.run().catch(console.error);
}

export { ZoneXKTGenerator };
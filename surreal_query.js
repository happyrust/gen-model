// SurrealDB direct query script
import { Surreal } from 'surrealdb.js';

async function debugBooleanOperations() {
    console.log("🔍 Connecting to SurrealDB to debug boolean operations...");
    
    // Try different port configurations
    const endpoints = [
        'http://127.0.0.1:8020/rpc',
        'http://127.0.0.1:8009/rpc', 
        'ws://127.0.0.1:8020/rpc',
        'ws://127.0.0.1:8009/rpc'
    ];
    
    let db = null;
    let connectedEndpoint = null;
    
    // Try to connect to each endpoint
    for (const endpoint of endpoints) {
        try {
            db = new Surreal(endpoint);
            await db.signin({
                username: 'root',
                password: 'root'
            });
            await db.use({
                ns: 'test',
                db: 'test'
            });
            connectedEndpoint = endpoint;
            console.log(`✅ Connected to: ${endpoint}`);
            break;
        } catch (err) {
            console.log(`❌ Failed to connect to ${endpoint}: ${err.message}`);
            continue;
        }
    }
    
    if (!db) {
        console.error("❌ Could not connect to any SurrealDB endpoint");
        return;
    }
    
    const testRefno = '17496_106028';
    console.log(`\n📊 Querying database for refno: ${testRefno}\n`);
    
    try {
        // 1. Check if pe record exists
        console.log("1️⃣ Checking pe record existence...");
        const peRecord = await db.query(`
            SELECT * FROM pe WHERE id = '${testRefno}' LIMIT 1
        `);
        console.log("Pe record:", peRecord[0]?.result?.[0] || "❌ Not found");
        
        // 2. Check total pe records
        console.log("\n2️⃣ Total pe records in database...");
        const totalPe = await db.query(`SELECT count() FROM pe`);
        console.log("Total pe records:", totalPe[0]?.result?.[0] || "❌ Query failed");
        
        // 3. Find similar refnos
        console.log("\n3️⃣ Looking for similar refnos...");
        const similarRefnos = await db.query(`
            SELECT id FROM pe WHERE id LIKE '${testRefno.split('_')[0]}%' LIMIT 10
        `);
        console.log("Similar refnos:", similarRefnos[0]?.result || "❌ No similar found");
        
        // 4. Check neg_relate for the test refno
        console.log("\n4️⃣ Checking neg_relate connections...");
        const negRelate = await db.query(`
            SELECT * FROM neg_relate WHERE out = '${testRefno}' OR in = '${testRefno}' LIMIT 5
        `);
        console.log("Neg relations:", negRelate[0]?.result?.length || 0, "records found");
        
        // 5. Check inst_relate records
        console.log("\n5️⃣ Checking inst_relate records...");
        const instRelateCount = await db.query(`
            SELECT count() FROM inst_relate
        `);
        console.log("Total inst_relate records:", instRelateCount[0]?.result?.[0] || "❌ Query failed");
        
        // 6. Check for the specific inst_relate
        console.log("\n6️⃣ Checking specific inst_relate for test refno...");
        const specificInstRelate = await db.query(`
            SELECT * FROM inst_relate WHERE in.id = '${testRefno}' LIMIT 1
        `);
        console.log("Specific inst_relate:", specificInstRelate[0]?.result?.[0] || "❌ Not found");
        
        // 7. Check what refnos do have inst_relate records
        console.log("\n7️⃣ Finding refnos that have inst_relate records...");
        const withInstRelate = await db.query(`
            SELECT in.id FROM inst_relate LIMIT 10
        `);
        console.log("Refnos with inst_relate:", withInstRelate[0]?.result?.slice(0, 5) || "❌ Query failed");
        
        // 8. Test boolean operation functions
        console.log("\n8️⃣ Testing negative entity query function...");
        try {
            const negEntities = await db.query(`
                RETURN fn::query_negative_entities('${testRefno}')
            `);
            console.log("Negative entities function result:", negEntities[0]?.result || "❌ Function failed");
        } catch (err) {
            console.log("❌ Negative entities function error:", err.message);
        }
        
    } catch (error) {
        console.error("❌ Query error:", error.message);
    } finally {
        await db.close();
        console.log(`\n🔌 Disconnected from: ${connectedEndpoint}`);
    }
}

debugBooleanOperations().catch(console.error);

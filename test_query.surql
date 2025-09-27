-- Query the element type and info for refno 17496_266203
SELECT refno, type, owner FROM pe:17496_266203;

-- Query its children
SELECT refno, type FROM pe WHERE owner = 17496_266203 LIMIT 10;

-- Query descendants with geometry data
SELECT refno, type FROM pe WHERE owner IN (
    SELECT refno FROM pe WHERE owner = 17496_266203
) AND type IN ['BOX', 'CYLI', 'CONE', 'DISH', 'ELBO', 'PYRA', 'RECT', 'SNOUT', 'TORUS'] LIMIT 20;

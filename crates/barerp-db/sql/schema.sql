CREATE TABLE IF NOT EXISTS "Table" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "number" INTEGER NOT NULL,
    "name" TEXT,
    "capacity" INTEGER NOT NULL DEFAULT 4,
    "status" TEXT NOT NULL DEFAULT 'AVAILABLE',
    "section" TEXT NOT NULL DEFAULT 'Salão Principal',
    "currentOrderId" TEXT,
    "customerCount" INTEGER NOT NULL DEFAULT 1,
    "customerName" TEXT,
    "openedAt" DATETIME,
    "mergedIntoTableId" TEXT,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL,
    "posX" REAL,
    "posY" REAL
);
CREATE TABLE IF NOT EXISTS "Order" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "orderNumber" INTEGER NOT NULL DEFAULT 1,
    "tableId" TEXT,
    "status" TEXT NOT NULL DEFAULT 'OPEN',
    "customerName" TEXT,
    "customerId" TEXT,
    "waiterId" TEXT,
    "waiterName" TEXT DEFAULT 'Garçom',
    "subtotal" REAL NOT NULL DEFAULT 0,
    "serviceFeeRate" REAL NOT NULL DEFAULT 0.10,
    "serviceFee" REAL NOT NULL DEFAULT 0,
    "discount" REAL NOT NULL DEFAULT 0,
    "total" REAL NOT NULL DEFAULT 0,
    "paidAmount" REAL NOT NULL DEFAULT 0,
    "isServiceFeeActive" BOOLEAN NOT NULL DEFAULT false,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL,
    "closedAt" DATETIME,
    CONSTRAINT "Order_tableId_fkey" FOREIGN KEY ("tableId") REFERENCES "Table" ("id") ON DELETE SET NULL ON UPDATE CASCADE,
    CONSTRAINT "Order_customerId_fkey" FOREIGN KEY ("customerId") REFERENCES "Customer" ("id") ON DELETE SET NULL ON UPDATE CASCADE,
    CONSTRAINT "Order_waiterId_fkey" FOREIGN KEY ("waiterId") REFERENCES "Waiter" ("id") ON DELETE SET NULL ON UPDATE CASCADE
);
CREATE TABLE IF NOT EXISTS "OrderItem" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "orderId" TEXT NOT NULL,
    "productId" TEXT NOT NULL,
    "quantity" INTEGER NOT NULL DEFAULT 1,
    "unitPrice" REAL NOT NULL,
    "totalPrice" REAL NOT NULL,
    "unitType" TEXT DEFAULT 'UNIT',
    "notes" TEXT,
    "paidQuantity" INTEGER NOT NULL DEFAULT 0,
    "modifiers" TEXT,
    "basePrice" REAL,
    "promotionName" TEXT,
    "kdsStatus" TEXT NOT NULL DEFAULT 'PENDING',
    "kdsStation" TEXT NOT NULL DEFAULT 'BAR',
    "addedAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL,
    CONSTRAINT "OrderItem_orderId_fkey" FOREIGN KEY ("orderId") REFERENCES "Order" ("id") ON DELETE CASCADE ON UPDATE CASCADE,
    CONSTRAINT "OrderItem_productId_fkey" FOREIGN KEY ("productId") REFERENCES "Product" ("id") ON DELETE RESTRICT ON UPDATE CASCADE
);
CREATE TABLE IF NOT EXISTS "Category" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "name" TEXT NOT NULL,
    "icon" TEXT DEFAULT 'Beer',
    "sortOrder" INTEGER NOT NULL DEFAULT 0,
    "codeStart" INTEGER DEFAULT 1001
);
CREATE TABLE IF NOT EXISTS "Product" (
    "code" TEXT,
    "ean" TEXT,
    "supplier" TEXT,
    "supplierId" TEXT,
    "ncm" TEXT,
    "cfop" TEXT,
    "cest" TEXT,
    "brand" TEXT,
    "id" TEXT NOT NULL PRIMARY KEY,
    "name" TEXT NOT NULL,
    "description" TEXT,
    "price" REAL NOT NULL,
    "costPrice" REAL,
    "targetMargin" REAL,
    "hasBoxPrice" BOOLEAN NOT NULL DEFAULT false,
    "boxQuantity" INTEGER DEFAULT 24,
    "boxPrice" REAL,
    "boxCostPrice" REAL,
    "boxEan" TEXT,
    "categoryId" TEXT NOT NULL,
    "kdsStation" TEXT NOT NULL DEFAULT 'BAR',
    "stock" INTEGER NOT NULL DEFAULT 100,
    "trackStock" BOOLEAN NOT NULL DEFAULT true,
    "minStock" INTEGER NOT NULL DEFAULT 10,
    "unit" TEXT NOT NULL DEFAULT 'un',
    "isActive" BOOLEAN NOT NULL DEFAULT true,
    "showInMenu" BOOLEAN NOT NULL DEFAULT true,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL,
    CONSTRAINT "Product_supplierId_fkey" FOREIGN KEY ("supplierId") REFERENCES "Supplier" ("id") ON DELETE SET NULL ON UPDATE CASCADE,
    CONSTRAINT "Product_categoryId_fkey" FOREIGN KEY ("categoryId") REFERENCES "Category" ("id") ON DELETE RESTRICT ON UPDATE CASCADE
);
CREATE TABLE IF NOT EXISTS "PriceHistory" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "productId" TEXT NOT NULL,
    "oldCostPrice" REAL,
    "newCostPrice" REAL,
    "oldPrice" REAL NOT NULL,
    "newPrice" REAL NOT NULL,
    "costDiff" REAL,
    "costPercent" REAL,
    "priceDiff" REAL NOT NULL,
    "pricePercent" REAL NOT NULL,
    "changedBy" TEXT NOT NULL DEFAULT 'Operador',
    "reason" TEXT NOT NULL,
    "nfeChave" TEXT,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT "PriceHistory_productId_fkey" FOREIGN KEY ("productId") REFERENCES "Product" ("id") ON DELETE CASCADE ON UPDATE CASCADE
);
CREATE TABLE IF NOT EXISTS "Payment" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "orderId" TEXT NOT NULL,
    "amount" REAL NOT NULL,
    "method" TEXT NOT NULL,
    "cashShiftId" TEXT,
    "notes" TEXT,
    "receivedAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT "Payment_orderId_fkey" FOREIGN KEY ("orderId") REFERENCES "Order" ("id") ON DELETE RESTRICT ON UPDATE CASCADE,
    CONSTRAINT "Payment_cashShiftId_fkey" FOREIGN KEY ("cashShiftId") REFERENCES "CashShift" ("id") ON DELETE SET NULL ON UPDATE CASCADE
);
CREATE TABLE IF NOT EXISTS "CashShift" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "openedAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "closedAt" DATETIME,
    "openedBy" TEXT NOT NULL DEFAULT 'Operador de Caixa',
    "closedBy" TEXT,
    "initialBalance" REAL NOT NULL DEFAULT 0,
    "finalBalance" REAL,
    "expectedBalance" REAL,
    "difference" REAL,
    "status" TEXT NOT NULL DEFAULT 'OPEN',
    "notes" TEXT
);
CREATE TABLE IF NOT EXISTS "CashTransaction" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "cashShiftId" TEXT NOT NULL,
    "type" TEXT NOT NULL,
    "amount" REAL NOT NULL,
    "reason" TEXT NOT NULL,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT "CashTransaction_cashShiftId_fkey" FOREIGN KEY ("cashShiftId") REFERENCES "CashShift" ("id") ON DELETE RESTRICT ON UPDATE CASCADE
);
CREATE TABLE IF NOT EXISTS "Waiter" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "name" TEXT NOT NULL,
    "code" TEXT,
    "commissionRate" REAL NOT NULL DEFAULT 0.10,
    "active" BOOLEAN NOT NULL DEFAULT true,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL
);
CREATE TABLE IF NOT EXISTS "SystemSettings" (
    "id" TEXT NOT NULL PRIMARY KEY DEFAULT 'default',
    "restaurantName" TEXT NOT NULL DEFAULT 'Meu Bar',
    "licenseEmail" TEXT,
    "licenseKey" TEXT,
    "machineId" TEXT,
    "isLicensed" BOOLEAN NOT NULL DEFAULT false,
    "defaultProfitMargin" REAL DEFAULT 50.0,
    "expiresAt" DATETIME,
    "lastVerifiedAt" DATETIME,
    "clientName" TEXT,
    "licenseStatus" TEXT DEFAULT 'UNLICENSED',
    "licenseServerUrl" TEXT,
    "developerContact" TEXT,
    "updatedAt" DATETIME NOT NULL
);
CREATE TABLE IF NOT EXISTS "ProductComponent" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "parentProductId" TEXT NOT NULL,
    "componentId" TEXT NOT NULL,
    "quantity" REAL NOT NULL,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL,
    CONSTRAINT "ProductComponent_parentProductId_fkey" FOREIGN KEY ("parentProductId") REFERENCES "Product" ("id") ON DELETE CASCADE ON UPDATE CASCADE,
    CONSTRAINT "ProductComponent_componentId_fkey" FOREIGN KEY ("componentId") REFERENCES "Product" ("id") ON DELETE RESTRICT ON UPDATE CASCADE
);
CREATE TABLE IF NOT EXISTS "AuditLog" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "action" TEXT NOT NULL,
    "description" TEXT NOT NULL,
    "userId" TEXT,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS "Customer" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "name" TEXT NOT NULL,
    "phone" TEXT,
    "document" TEXT,
    "notes" TEXT,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL,
    "creditTabBalance" REAL NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS "FiscalSettings" (
    "cep" TEXT,
    "logradouro" TEXT,
    "numero" TEXT,
    "bairro" TEXT,
    "municipio" TEXT,
    "uf" TEXT,
    "razaoSocial" TEXT,
    "nomeFantasia" TEXT,
    "environment" TEXT DEFAULT 'homologacao',
    "serieNfce" TEXT DEFAULT '1',
    "proximoNumeroNfce" INTEGER DEFAULT 1,
    "serieNfe" TEXT DEFAULT '1',
    "proximoNumeroNfe" INTEGER DEFAULT 1,
    "id" TEXT NOT NULL PRIMARY KEY DEFAULT 'default',
    "apiToken" TEXT,
    "cnpj" TEXT,
    "ie" TEXT,
    "crt" TEXT,
    "cscId" TEXT,
    "cscSecret" TEXT,
    "addressInfo" TEXT,
    "updatedAt" DATETIME NOT NULL
);
CREATE TABLE IF NOT EXISTS "NotaRecebida" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "chave" TEXT NOT NULL,
    "emitente" TEXT,
    "cnpjEmitente" TEXT,
    "numero" TEXT,
    "serie" TEXT,
    "dataEmissao" TEXT,
    "valorTotal" REAL,
    "status" TEXT NOT NULL DEFAULT 'recebida',
    "xmlContent" TEXT,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS "NotaEmitida" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "referencia" TEXT NOT NULL,
    "chave" TEXT,
    "numero" TEXT,
    "serie" TEXT,
    "dataEmissao" TEXT,
    "valorTotal" REAL,
    "status" TEXT NOT NULL DEFAULT 'autorizado',
    "xmlUrl" TEXT,
    "pdfUrl" TEXT,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS "Supplier" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "name" TEXT NOT NULL,
    "tradeName" TEXT,
    "document" TEXT,
    "ie" TEXT,
    "phone" TEXT,
    "email" TEXT,
    "city" TEXT,
    "state" TEXT,
    "address" TEXT,
    "contactName" TEXT,
    "notes" TEXT,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL
);
CREATE TABLE IF NOT EXISTS "Staff" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "name" TEXT NOT NULL,
    "role" TEXT NOT NULL DEFAULT 'OPERADOR',
    "password" TEXT NOT NULL,
    "permissions" TEXT NOT NULL DEFAULT '["tables","cash"]',
    "active" BOOLEAN NOT NULL DEFAULT true,
    "failedAttempts" INTEGER NOT NULL DEFAULT 0,
    "lockoutUntil" DATETIME,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL
);
CREATE TABLE IF NOT EXISTS "ModifierGroup" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "name" TEXT NOT NULL,
    "minSelect" INTEGER NOT NULL DEFAULT 0,
    "maxSelect" INTEGER NOT NULL DEFAULT 1,
    "sortOrder" INTEGER NOT NULL DEFAULT 0,
    "isActive" BOOLEAN NOT NULL DEFAULT true,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL
);
CREATE TABLE IF NOT EXISTS "ModifierOption" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "groupId" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "price" REAL NOT NULL DEFAULT 0,
    "sortOrder" INTEGER NOT NULL DEFAULT 0,
    "isActive" BOOLEAN NOT NULL DEFAULT true,
    CONSTRAINT "ModifierOption_groupId_fkey" FOREIGN KEY ("groupId") REFERENCES "ModifierGroup" ("id") ON DELETE CASCADE ON UPDATE CASCADE
);
CREATE TABLE IF NOT EXISTS "ProductModifierGroup" (
    "productId" TEXT NOT NULL,
    "groupId" TEXT NOT NULL,
    "sortOrder" INTEGER NOT NULL DEFAULT 0,

    PRIMARY KEY ("productId", "groupId"),
    CONSTRAINT "ProductModifierGroup_productId_fkey" FOREIGN KEY ("productId") REFERENCES "Product" ("id") ON DELETE CASCADE ON UPDATE CASCADE,
    CONSTRAINT "ProductModifierGroup_groupId_fkey" FOREIGN KEY ("groupId") REFERENCES "ModifierGroup" ("id") ON DELETE CASCADE ON UPDATE CASCADE
);
CREATE TABLE IF NOT EXISTS "Promotion" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "name" TEXT NOT NULL,
    "isActive" BOOLEAN NOT NULL DEFAULT true,
    "daysOfWeek" TEXT NOT NULL DEFAULT '0,1,2,3,4,5,6',
    "startTime" TEXT NOT NULL DEFAULT '00:00',
    "endTime" TEXT NOT NULL DEFAULT '23:59',
    "discountType" TEXT NOT NULL DEFAULT 'PERCENT',
    "value" REAL NOT NULL,
    "productIds" TEXT NOT NULL DEFAULT '[]',
    "categoryIds" TEXT NOT NULL DEFAULT '[]',
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL
);
CREATE TABLE IF NOT EXISTS "ProductImage" (
    "productId" TEXT NOT NULL PRIMARY KEY,
    "data" BLOB NOT NULL,
    "mimeType" TEXT NOT NULL,
    "hash" TEXT NOT NULL,
    "publishedHash" TEXT,
    "updatedAt" DATETIME NOT NULL
);
CREATE TABLE IF NOT EXISTS "DigitalMenuSettings" (
    "id" TEXT NOT NULL PRIMARY KEY DEFAULT 'default',
    "enabled" BOOLEAN NOT NULL DEFAULT false,
    "siteUrl" TEXT,
    "storeId" TEXT,
    "publishSecret" TEXT,
    "title" TEXT,
    "message" TEXT,
    "hiddenCategoryIds" TEXT NOT NULL DEFAULT '[]',
    "hideSoldOut" BOOLEAN NOT NULL DEFAULT false,
    "ignoreStock" BOOLEAN NOT NULL DEFAULT false,
    "lastPublishedAt" DATETIME,
    "lastHash" TEXT,
    "lastError" TEXT,
    "updatedAt" DATETIME NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS "Table_number_key" ON "Table"("number");
CREATE UNIQUE INDEX IF NOT EXISTS "Waiter_name_key" ON "Waiter"("name");
CREATE UNIQUE INDEX IF NOT EXISTS "ProductComponent_parentProductId_componentId_key" ON "ProductComponent"("parentProductId", "componentId");
CREATE UNIQUE INDEX IF NOT EXISTS "NotaRecebida_chave_key" ON "NotaRecebida"("chave");
CREATE UNIQUE INDEX IF NOT EXISTS "NotaEmitida_referencia_key" ON "NotaEmitida"("referencia");
CREATE UNIQUE INDEX IF NOT EXISTS "Supplier_document_key" ON "Supplier"("document");

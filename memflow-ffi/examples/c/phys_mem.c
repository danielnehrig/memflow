#include "memflow.h"

#include <stdio.h>

int main(int argc, char *argv[]) {
	// enable debug level logging
	log_init(3);

	Inventory *inv = inventory_scan();
	printf("inv: %p\n", inv);

	const char *conn_name = argc > 1 ? argv[1] : "kvm";
	const char *conn_arg = argc > 2 ? argv[2] : "";

	ConnectorInstanceArcBox conn;
	if (!inventory_create_connector(inv, conn_name, conn_arg, &conn)) {
		for (int i = 0; i < 1000 * 1000; i++) {
			uint8_t buffer[0x1000];

			ConnectorInstanceArcBox cloned = connectorinstance_clone(&conn);

			connectorinstance_arc_box_drop(cloned);

			MemoryViewArcBox phys_view = connectorinstance_phys_view(&conn);

			// regular read_into
			read_raw_into(&phys_view, 0x1000 + i, MUT_SLICE(u8, buffer, sizeof(buffer)));

			// read multiple
			ReadData read_data = {0x1000 + i, {buffer, sizeof(buffer)}};
			read_raw_list(&phys_view, MUT_SLICE(ReadData, &read_data, 1));

			printf("Read: %lx\n", *(uint64_t *)buffer);

			memoryview_arc_box_drop(phys_view);
		}

		connectorinstance_arc_box_drop(conn);
		printf("conn dropped!\n");
	}

	inventory_free(inv);
	printf("inv freed!\n");

	return 0;
}

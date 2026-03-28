/*******************************************************************************
 *
 * University of Illinois/NCSA
 * Open Source License
 *
 * Copyright (c) 2018 Advanced Micro Devices, Inc. All Rights Reserved.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * with the Software without restriction, including without limitation the
 * rights to use, copy, modify, merge, publish, distribute, sublicense, and/or
 * sell copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 *     * Redistributions of source code must retain the above copyright notice,
 *       this list of conditions and the following disclaimers.
 *
 *     * Redistributions in binary form must reproduce the above copyright
 *       notice, this list of conditions and the following disclaimers in the
 *       documentation and/or other materials provided with the distribution.
 *
 *     * Neither the names of Advanced Micro Devices, Inc. nor the names of its
 *       contributors may be used to endorse or promote products derived from
 *       this Software without specific prior written permission.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * CONTRIBUTORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS WITH
 * THE SOFTWARE.
 *
 ******************************************************************************/

#include "amd_comgr.h"
#include "common.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct container {
  char *Data;
  int Sz;
} container_t;

void collectSymbolizedString(const char *Input, void *Data) {
  int Sz = strlen(Input);
  container_t *Ptr = (container_t *)Data;
  Ptr->Data = (char *)malloc(Sz + 1);
  Ptr->Data[Sz] = '\0';
  Ptr->Sz = Sz;
  memcpy(Ptr->Data, Input, Sz);
}

void testSymbolizedString(container_t *SymbolContainer) {

  char *SymbolStr = SymbolContainer->Data;
  char* SpacePos = strchr(SymbolStr, ' ');
  if (SpacePos == NULL) {
    printf("Expected spaces in %s\n", SymbolStr);
    exit(0);
  }

  size_t FuncNameSize = SpacePos - SymbolStr;
  char *FuncName = (char*) malloc(sizeof(char) * (FuncNameSize + 1));

  if (!SymbolStr) {
    printf("Failed, symbol_str NULL\n");
    exit(0);
  }

  strncpy(FuncName, SymbolStr, FuncNameSize);
  FuncName[FuncNameSize] = '\0';

  if (strcmp(FuncName, "bazzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz")) {
    printf("mismatch:\n");
    printf("expected symbolized function name: bazzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz\n");
    printf("actual symbolized function name: %s\n", FuncName);
    exit(0);
  }

  printf("symbolized string is %s", SymbolStr);
  free(FuncName);
  free(SymbolStr);

  return;
}

int main(int argc, char *argv[]) {
  size_t Size;
  char *Buf;
  amd_comgr_data_t DataIn;
  amd_comgr_status_t Status;
  amd_comgr_symbolizer_info_t Symbolizer;
  container_t UserData;

  // Read input file
  Size = setBuf(TEST_OBJ_DIR "/shared-debug.so", &Buf);

  // Create data object
  {
    printf("Test create input data set\n");
    Status = amd_comgr_create_data(AMD_COMGR_DATA_KIND_EXECUTABLE, &DataIn);
    checkError(Status, "amd_comgr_create_data");
    Status = amd_comgr_set_data(DataIn, Size, Buf);
    checkError(Status, "amd_comgr_set_data");
    Status = amd_comgr_set_data_name(DataIn, "shared-debug.so");
    checkError(Status, "amd_comgr_set_data_name");
  }

  // Create symbolizer info and symbolize
  {
    printf("Test create symbolizer info\n");
    Status = amd_comgr_create_symbolizer_info(DataIn, &collectSymbolizedString,
                                              &Symbolizer);
    checkError(Status, "amd_comgr_create_symbolizer_info");
    // Use this command to get valid address
    // llvm-objdump --triple=amdgcn-amd-amdhsa -l --mcpu=gfx900 --disassemble --source shared.so
    int address = 5896;
    Status = amd_comgr_symbolize(Symbolizer, address, 1, (void *)&UserData);
    checkError(Status, "amd_comgr_symbolize");

    testSymbolizedString(&UserData);
  }

  // Destroy symbolizer info
  {
    printf("Test destroy symbolizer info\n");
    Status = amd_comgr_destroy_symbolizer_info(Symbolizer);
    checkError(Status, "amd_comgr_destroy_symbolizer_info");
    Status = amd_comgr_release_data(DataIn);
    checkError(Status, "amd_comgr_release_data");
    free(Buf);
  }

  return 0;
}
